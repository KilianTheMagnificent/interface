import { describe, it, expect } from "vitest";
import type {
  EngineEvent, EngineId, SessionStatus, SessionSummary,
} from "../shared/engine";
import type { TerminalSummary } from "../shared/ipc";
// The dock badge's own arithmetic, imported rather than restated: the pill and
// the badge claiming one fact is only enforceable if one test sees both.
import { waitingCount as dockBadgeCount } from "../main/badge";
import { LIVE_COMPACTION_LOW_WATER } from "./live-events";
import {
  MAX_LIVE_EVENTS_PER_SESSION, appReducer, initialState, type AppState,
} from "./store";
import {
  attentionAnnouncement, attentionSessions,
  barWidth, buildSidebarGroups,
  buildSidebarView, sessionSurfaceOf,
  buildUsageView, configInForce, engineCan, engineOpsMap, fastModeInForce, formatReset,
  importProgressLabel,
  importSummary, managedWorktreeName, OTHER_PROJECT, OTHER_PROJECT_NAME,
  pendingApproval,
  sessionAgeLabel,
  permissionPicker, queryTerms, sessionActivity, sessionMatches, sidebarEmptyState,
  sidebarFilterLabel, sidebarRowMeta,
  terminalHasActiveWork,
  TRIVIAL_SESSION_BYTES, TRIVIAL_SESSION_KIB, formatTurnElapsed, rememberTurnStart,
  stampTurnClock, turnProgress,
  usageFreshness, usageRailReading, usageStripItems, usageStripSummary, usageStripTone,
  USAGE_STALE_MS, needsAttention, withoutDefaultPermissionMode,
  workspaceSidebarBadge, workspaceSidebarDone, workspaceSidebarPips, workspaceSidebarProjectPath,
  workspaceSidebarTitle,
  workspacesActiveFirst,
} from "./viewmodels";
import type { SidebarGroup, SidebarRow } from "./viewmodels";
// The sidebar's own attention predicate, imported rather than restated: the
// pill and the pinned section claiming compatible things is only enforceable if
// one test sees both.
import { isAttentionRow } from "./components/Sidebar";
// The shared status table, imported rather than restated: the second line must
// print the same word for a state as every other surface in the app.
import { statusWord } from "./components/status-words";

// `projectRoot` and `origin` are deliberately ABSENT by default, because that is
// what every session on this machine looks like today: nothing is tagged, and a
// session outside a repository has no root. Tests that want a project or the
// Interface marker say so.
function session(engine: EngineId, sessionId: string,
  over: Partial<SessionSummary> = {}): SessionSummary {
  return {
    engine, sessionId, title: `${sessionId} title`, cwd: "/tmp/proj",
    status: "idle", cloud: false,
    lastActivity: "2026-08-11T10:00:00.000Z",
    firstSeen: "2026-08-11T09:00:00.000Z", archived: false, ...over,
  };
}

function terminal(id: string, over: Partial<TerminalSummary> = {}): TerminalSummary {
  return {
    id,
    generation: 1,
    title: `${id} terminal`,
    root: "/tmp/proj",
    cwd: "/tmp/proj",
    state: "running",
    size: { columns: 80, rows: 24 },
    createdAt: "2026-08-11T09:00:00.000Z",
    updatedAt: "2026-08-11T10:00:00.000Z",
    ...over,
  };
}

function state(over: Partial<AppState> = {}): AppState {
  return { ...initialState, ...over };
}

describe("appReducer — event", () => {
  it("coalesces adjacent provider chunks without touching the other logs", () => {
    const prev = state({
      events: {
        "claude#a": [{ kind: "message.delta", text: "one" }],
        "claude#b": [{ kind: "message.delta", text: "other" }],
      },
    });
    const next = appReducer(prev,
      { type: "event", key: "claude#a", evt: { kind: "message.delta", text: "two" } });

    expect(next.events["claude#a"]).toEqual([
      { kind: "message.delta", text: "onetwo" },
    ]);
    expect(next.events["claude#b"]).toBe(prev.events["claude#b"]);
    expect(prev.events["claude#a"]).toHaveLength(1);   // input not mutated
    expect(next).not.toBe(prev);
  });

  it("starts a log for a key never seen before", () => {
    const next = appReducer(state(),
      { type: "event", key: "codex#z", evt: { kind: "thinking.signal", active: true } });
    expect(next.events["codex#z"]).toEqual([{ kind: "thinking.signal", active: true }]);
  });

  it("batches multiple IPC events into one state update while preserving key order", () => {
    const next = appReducer(state(), { type: "events", items: [
      { key: "claude#a", evt: { kind: "message.delta", text: "one" } },
      { key: "codex#b", evt: { kind: "status.changed", status: "working" } },
      { key: "claude#a", evt: { kind: "message.delta", text: " two" } },
    ] });
    expect(next.events["claude#a"]).toEqual([{ kind: "message.delta", text: "one two" }]);
    expect(next.events["codex#b"]).toEqual([{ kind: "status.changed", status: "working" }]);
  });

  it("keeps command catalogues session-scoped, replaceable, and out of transcripts", () => {
    const first = appReducer(state(), { type: "events", items: [
      { key: "claude#a", evt: { kind: "commands.available", commands: [
        { name: "review", description: "Review changes" },
      ] } },
      { key: "codex#b", evt: { kind: "commands.available", commands: [
        { name: "compact", description: "Compact context" },
      ] } },
    ] });
    expect(first.events).toEqual({});
    expect(first.providerCommands["claude#a"]?.map(command => command.name)).toEqual(["review"]);
    expect(first.providerCommands["codex#b"]?.map(command => command.name)).toEqual(["compact"]);

    const replaced = appReducer(first, { type: "event", key: "claude#a",
      evt: { kind: "commands.available", commands: [] } });
    expect(replaced.providerCommands["claude#a"]).toEqual([]);
    expect(replaced.providerCommands["codex#b"]).toBe(first.providerCommands["codex#b"]);

    const invalidated = appReducer(replaced, { type: "event", key: "claude#a",
      evt: { kind: "commands.invalidated" } });
    expect(invalidated.providerCommands["claude#a"]).toEqual([]);
    expect(invalidated.providerCommands["codex#b"]).toBe(first.providerCommands["codex#b"]);
    expect(invalidated.events["claude#a"]).toBeUndefined();

    const closed = appReducer(invalidated, { type: "event", key: "codex#b",
      evt: { kind: "session.closed" } });
    expect(closed.providerCommands["codex#b"]).toBeUndefined();
    expect(closed.events["codex#b"]).toEqual([{ kind: "session.closed" }]);
  });

  it("compacts a sustained 50,000-chunk stream into one semantic message", () => {
    const items = Array.from({ length: 50_000 }, () => ({
      key: "claude#soak",
      evt: { kind: "message.delta", text: "x" } as EngineEvent,
    }));
    const next = appReducer(state(), { type: "events", items });
    expect(next.events["claude#soak"]).toHaveLength(1);
    expect(next.events["claude#soak"][0]).toMatchObject({
      kind: "message.delta", text: "x".repeat(50_000),
    });
  });

  it("drops duplicate thinking state and bounds semantic live history", () => {
    const thinking: EngineEvent = { kind: "thinking.signal", active: true };
    const once = appReducer(state(), { type: "event", key: "claude#a", evt: thinking });
    expect(appReducer(once, { type: "event", key: "claude#a", evt: thinking })).toBe(once);

    let bounded = state();
    for (let i = 0; i <= MAX_LIVE_EVENTS_PER_SESSION; i++) {
      bounded = appReducer(bounded, { type: "event", key: "claude#a",
        evt: { kind: "notice", tone: "info", message: String(i) } });
    }
    const log = bounded.events["claude#a"];
    // One event over the cap compacts to the low-water mark, not to the cap.
    expect(log).toHaveLength(Math.floor(MAX_LIVE_EVENTS_PER_SESSION * LIVE_COMPACTION_LOW_WATER));
    expect(log[0]).toMatchObject({ kind: "notice", tone: "warning" });
    expect(log.at(-1)).toMatchObject({ kind: "notice", message: String(MAX_LIVE_EVENTS_PER_SESSION) });
  });

  it("leaves everything but the event log alone", () => {
    const prev = state({
      sessions: [session("claude", "a")], selectedKey: "claude#a", composerOpen: true,
      engineHealth: [{ engine: "claude", ok: true, version: "1.0.0" }],
    });
    const next = appReducer(prev,
      { type: "event", key: "claude#a", evt: { kind: "status.changed", status: "working" } });
    expect(next.sessions).toBe(prev.sessions);
    expect(next.selectedKey).toBe("claude#a");
    expect(next.composerOpen).toBe(true);
    expect(next.engineHealth).toBe(prev.engineHealth);
  });
});

describe("appReducer — registry", () => {
  it("replaces the session list", () => {
    const prev = state({ sessions: [session("claude", "old")] });
    const sessions = [session("claude", "a"), session("codex", "b")];
    const next = appReducer(prev, { type: "registry", sessions });
    expect(next.sessions).toEqual(sessions);
    expect(prev.sessions).toHaveLength(1);            // input not mutated
  });

  it("keeps the selection when the selected key is still listed", () => {
    const prev = state({ sessions: [session("claude", "a")], selectedKey: "claude#a" });
    const next = appReducer(prev,
      { type: "registry", sessions: [session("claude", "a"), session("claude", "b")] });
    expect(next.selectedKey).toBe("claude#a");
  });

  it("clears the selection when the selected key is gone", () => {
    const prev = state({ sessions: [session("claude", "a")], selectedKey: "claude#a" });
    const next = appReducer(prev, { type: "registry", sessions: [session("claude", "b")] });
    expect(next.selectedKey).toBeNull();
  });

  it("does not match a key from a different engine", () => {
    const prev = state({ sessions: [session("claude", "a")], selectedKey: "claude#a" });
    const next = appReducer(prev, { type: "registry", sessions: [session("codex", "a")] });
    expect(next.selectedKey).toBeNull();
  });

  it("keeps the event logs across a registry replacement", () => {
    const prev = state({
      selectedKey: "claude#a",
      sessions: [session("claude", "a")],
      events: { "claude#a": [{ kind: "message.delta", text: "one" }] },
    });
    const next = appReducer(prev, { type: "registry", sessions: [session("claude", "a")] });
    expect(next.events).toBe(prev.events);
  });

  it("keeps unchanged rows, and the whole state when nothing changed", () => {
    const prev = state({
      selectedKey: "claude#a",
      sessions: [session("claude", "a"), session("codex", "b")],
    });
    // A focus refresh: equal rows, freshly deserialized.
    const same = appReducer(prev, { type: "registry",
      sessions: [session("claude", "a"), session("codex", "b")] });
    expect(same).toBe(prev);

    const moved = appReducer(prev, { type: "registry", sessions: [
      session("codex", "b", { status: "working", lastActivity: "2026-08-11T11:00:00.000Z" }),
      session("claude", "a"),
    ] });
    expect(moved.sessions[0]).not.toBe(prev.sessions[1]);
    expect(moved.sessions[0].status).toBe("working");
    expect(moved.sessions[1]).toBe(prev.sessions[0]);

    // Same rows in a new order is a change of the list, not of the rows.
    const reordered = appReducer(prev, { type: "registry",
      sessions: [session("codex", "b"), session("claude", "a")] });
    expect(reordered).not.toBe(prev);
    expect(reordered.sessions[0]).toBe(prev.sessions[1]);
    expect(reordered.sessions[1]).toBe(prev.sessions[0]);
  });
});

describe("appReducer — authoritative drop", () => {
  it("reclaims only the deleted session's event log and command catalogue", () => {
    const prev = state({
      sessions: [session("claude", "a"), session("codex", "b")],
      selectedKey: "claude#a",
      events: {
        "claude#a": [{ kind: "message.delta", text: "large transcript" }],
        "codex#b": [{ kind: "message.delta", text: "keep" }],
      },
      providerCommands: {
        "claude#a": [{ name: "review", description: "Review changes" }],
        "codex#b": [{ name: "compact", description: "Compact context" }],
      },
    });

    const next = appReducer(prev, { type: "drop", key: "claude#a" });
    expect(next.events["claude#a"]).toBeUndefined();
    expect(next.providerCommands["claude#a"]).toBeUndefined();
    expect(next.events["codex#b"]).toBe(prev.events["codex#b"]);
    expect(next.providerCommands["codex#b"]).toBe(prev.providerCommands["codex#b"]);
    expect(next.sessions).toBe(prev.sessions);
    expect(next.selectedKey).toBe("claude#a");
    expect(prev.events["claude#a"]).toHaveLength(1);
  });

  it("preserves reducer identity when the key has no retained state", () => {
    const prev = state();
    expect(appReducer(prev, { type: "drop", key: "claude#missing" })).toBe(prev);
  });
});

describe("appReducer — select, composer, health", () => {
  it("select sets and clears the selected key", () => {
    const chosen = appReducer(state(), { type: "select", key: "claude#a" });
    expect(chosen.selectedKey).toBe("claude#a");
    expect(appReducer(chosen, { type: "select", key: null }).selectedKey).toBeNull();
  });

  it("composer opens and closes", () => {
    const open = appReducer(state(), { type: "composer", open: true });
    expect(open.composerOpen).toBe(true);
    expect(appReducer(open, { type: "composer", open: false }).composerOpen).toBe(false);
  });

  it("health replaces the engine list", () => {
    const engines = [
      { engine: "claude" as EngineId, ok: true, version: "1.2.3" },
      { engine: "codex" as EngineId, ok: false, error: "not installed" },
    ];
    const prev = state({ engineHealth: [{ engine: "claude", ok: false, error: "stale" }] });
    const next = appReducer(prev, { type: "health", engines });
    expect(next.engineHealth).toEqual(engines);
    expect(prev.engineHealth).toEqual([{ engine: "claude", ok: false, error: "stale" }]);
  });

  it("returns the same state when select, composer or health changes nothing", () => {
    // A pane click re-selects its session and closes a closed composer; a
    // window focus re-probes the same engines. None of them may re-render App.
    const prev = state({ selectedKey: "claude#a", composerOpen: false,
      engineHealth: [{ engine: "claude", ok: true, version: "1.2.3" }] });
    expect(appReducer(prev, { type: "select", key: "claude#a" })).toBe(prev);
    expect(appReducer(prev, { type: "composer", open: false })).toBe(prev);
    expect(appReducer(prev, { type: "health",
      engines: [{ engine: "claude", ok: true, version: "1.2.3" }] })).toBe(prev);
    expect(appReducer(prev, { type: "health",
      engines: [{ engine: "claude", ok: true, version: "1.2.4" }] })).not.toBe(prev);
    expect(appReducer(prev, { type: "select", key: null })).not.toBe(prev);
    expect(appReducer(prev, { type: "composer", open: true })).not.toBe(prev);
  });

  it("initialState opens on a ready new-session prompt", () => {
    expect(initialState).toEqual({
      sessions: [], selectedKey: null, events: {}, providerCommands: {},
      engineHealth: [], composerOpen: true,
    });
  });
});

/** Every sidebar row across its project groups. */
const sidebarRows = (sessions: SessionSummary[], selected: string | null): SidebarRow[] =>
  buildSidebarGroups(sessions, selected).flatMap(group => group.rows);

describe("sidebar rows", () => {
  const sessions = [
    session("claude", "a", { title: "Refactor", status: "working" }),
    session("codex", "b", { title: "Docs", status: "waiting", cloud: true }),
    session("claude", "c", { title: "Tests", status: "failed" }),
  ];

  it("keeps one row per session, in listing order among equally recent rows", () => {
    expect(sidebarRows(sessions, null).map(r => r.key))
      .toEqual(["claude#a", "codex#b", "claude#c"]);
  });

  it("sets selected on exactly one row", () => {
    const rows = sidebarRows(sessions, "codex#b");
    expect(rows.filter(r => r.selected)).toHaveLength(1);
    expect(rows.find(r => r.selected)?.key).toBe("codex#b");
  });

  it("selects nothing when there is no selection or the key is unknown", () => {
    expect(sidebarRows(sessions, null).some(r => r.selected)).toBe(false);
    expect(sidebarRows(sessions, "claude#gone").some(r => r.selected)).toBe(false);
  });

  it("badge equals status", () => {
    const statuses: SessionStatus[] = ["working", "waiting", "idle", "failed"];
    const rows = sidebarRows(statuses.map((s, i) => session("claude", `s${i}`, { status: s })),
      null);
    expect(rows.map(r => r.badge)).toEqual(statuses);
    expect(rows.every(r => r.badge === r.status)).toBe(true);
  });

  it("carries title, engine and cloud through", () => {
    const [claudeRow, codexRow] = sidebarRows(sessions, null);
    expect(claudeRow).toEqual({
      key: "claude#a", title: "Refactor", engine: "claude", status: "working",
      badge: "working", cloud: false, selected: false,
      lastActivity: "2026-08-11T10:00:00.000Z", external: true,
    });
    expect(codexRow.engine).toBe("codex");
    expect(codexRow.cloud).toBe(true);
  });

  it("marks a row external unless the session says origin app", () => {
    // Same rule the origin filter applies: ABSENT is "store", never "app".
    const rows = sidebarRows([
      session("claude", "own", { origin: "app" }),
      session("claude", "cli", { origin: "store" }),
      session("claude", "unknown"),
    ], null);
    expect(rows.map(r => r.external)).toEqual([false, true, true]);
  });

  it("is empty for an empty listing", () => {
    expect(sidebarRows([], "claude#a")).toEqual([]);
  });

  it("compacts recency the way a sidebar age column should", () => {
    const now = Date.parse("2026-08-17T12:00:00.000Z");
    expect(sessionAgeLabel("2026-08-17T11:59:30.000Z", now)).toBe("now");
    expect(sessionAgeLabel("2026-08-17T11:05:00.000Z", now)).toBe("55m");
    expect(sessionAgeLabel("2026-08-17T09:00:00.000Z", now)).toBe("3h");
    expect(sessionAgeLabel("2026-08-15T12:00:00.000Z", now)).toBe("2d");
    expect(sessionAgeLabel("not-a-date", now)).toBe("");
  });
});

describe("buildSidebarGroups — grouping by project root", () => {
  it("is empty for an empty listing", () => {
    expect(buildSidebarGroups([], "claude#a")).toEqual([]);
  });

  it("collapses every cwd inside one repository into that one group", () => {
    // THE reason grouping moved off cwd: these three ran in three directories
    // and are one project. Keyed on cwd this was three groups, two of which were
    // junk ("src", "renderer").
    const groups = buildSidebarGroups([
      session("claude", "a", { cwd: "/work/interface", projectRoot: "/work/interface" }),
      session("claude", "b", { cwd: "/work/interface/src", projectRoot: "/work/interface" }),
      session("codex", "c", {
        cwd: "/work/interface/src/renderer", projectRoot: "/work/interface",
      }),
    ], null);
    expect(groups).toHaveLength(1);
    expect(groups[0].projectPath).toBe("/work/interface");
    expect(groups[0].projectName).toBe("interface");
    expect(groups[0].rows.map(r => r.key)).toEqual(["claude#a", "claude#b", "codex#c"]);
    expect(groups[0].counts.total).toBe(3);
  });

  it("ignores cwd entirely — the root decides, even for identical cwds", () => {
    // A cwd can be shared by two roots (a symlinked or re-cloned checkout). The
    // root is the only key.
    const groups = buildSidebarGroups([
      session("claude", "a", { cwd: "/shared", projectRoot: "/work/one" }),
      session("claude", "b", { cwd: "/shared", projectRoot: "/work/two" }),
    ], null);
    expect(groups.map(g => g.projectPath).sort()).toEqual(["/work/one", "/work/two"]);
  });

  it("splits many projects, one group each, naming by basename", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/work/one" }),
      session("claude", "b", { projectRoot: "/work/two" }),
      session("claude", "c", { projectRoot: "/elsewhere/three" }),
    ], null);
    expect(groups.map(g => g.projectPath).sort())
      .toEqual(["/elsewhere/three", "/work/one", "/work/two"]);
    expect(groups.map(g => g.projectName).sort()).toEqual(["one", "three", "two"]);
    expect(groups.every(g => g.rows.length === 1)).toBe(true);
  });

  it("keeps colliding basenames in separate groups with distinct paths", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/a/web" }),
      session("claude", "b", { projectRoot: "/b/web" }),
    ], null);
    expect(groups).toHaveLength(2);
    expect(groups.map(g => g.projectName)).toEqual(["web", "web"]);
    expect(groups.map(g => g.projectPath).sort()).toEqual(["/a/web", "/b/web"]);
  });

  it("groups a trailing-slash project root with the same root without one", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/a/web" }),
      session("claude", "b", { projectRoot: "/a/web/" }),
      session("claude", "c", { projectRoot: "/a/web//" }),
    ], null);
    expect(groups).toHaveLength(1);
    expect(groups[0].projectPath).toBe("/a/web");        // stripped, not otherwise rewritten
    expect(groups[0].projectName).toBe("web");
    expect(groups[0].counts.total).toBe(3);
  });

  it("names a repository at the filesystem root by its own path", () => {
    // "/" has no nameable segment. It is still a real root, so it is a real
    // group — and it must not be confused with, or merged into, "Other".
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/" }),
      session("claude", "b"),                            // no root at all
    ], null);
    expect(groups.map(g => g.projectPath)).toEqual(["/", OTHER_PROJECT]);
    expect(groups.map(g => g.projectName)).toEqual(["/", OTHER_PROJECT_NAME]);
  });
});

describe("buildSidebarGroups — the Other bucket", () => {
  it("puts every rootless session in ONE group, whatever its cwd", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { cwd: "/Users/me" }),
      session("claude", "b", { cwd: "/tmp/scratch" }),
      session("codex", "c", { cwd: "/Users/me/Projects" }),
    ], null);
    expect(groups).toHaveLength(1);
    expect(groups[0].projectPath).toBe(OTHER_PROJECT);
    expect(groups[0].projectName).toBe(OTHER_PROJECT_NAME);
    expect(groups[0].rows.map(r => r.key)).toEqual(["claude#a", "claude#b", "codex#c"]);
    expect(groups[0].counts.total).toBe(3);
  });

  it("treats an empty project root as absent", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "" }),
      session("claude", "b"),
    ], null);
    expect(groups).toHaveLength(1);
    expect(groups[0].projectPath).toBe(OTHER_PROJECT);
  });

  it("holds nothing that has a root", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/work/one" }),
      session("claude", "b"),
      session("claude", "c", { projectRoot: "/work/one" }),
    ], null);
    const byPath = new Map(groups.map(g => [g.projectPath, g]));
    expect(byPath.get("/work/one")?.rows.map(r => r.key)).toEqual(["claude#a", "claude#c"]);
    expect(byPath.get(OTHER_PROJECT)?.rows.map(r => r.key)).toEqual(["claude#b"]);
  });

  it("uses an identity no real path can equal", () => {
    // A NUL byte is illegal in a path on every filesystem this app runs on, so
    // the sentinel cannot be produced by the main process's root detection.
    expect(OTHER_PROJECT).toContain("\u0000");
    expect(OTHER_PROJECT.startsWith("/")).toBe(false);
  });

  it("does not swallow a repository that is literally called Other", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/Users/me/Other" }),
      session("claude", "b"),
    ], null);
    expect(groups).toHaveLength(2);
    const real = groups.find(g => g.projectPath === "/Users/me/Other");
    expect(real?.projectName).toBe("Other");             // same label, different identity
    expect(real?.rows.map(r => r.key)).toEqual(["claude#a"]);
    expect(groups.find(g => g.projectPath === OTHER_PROJECT)?.rows.map(r => r.key))
      .toEqual(["claude#b"]);
  });

  it("sorts last even when it holds the only waiting session", () => {
    // The one deliberate exception to the urgency band: a leftovers bin never
    // pushes a real project down the pane. The group header still shows the
    // attention dot, so nothing is concealed.
    const groups = buildSidebarGroups([
      session("claude", "a", { status: "waiting", lastActivity: "2026-08-11T23:00:00.000Z" }),
      session("claude", "b", {
        projectRoot: "/p/quiet", status: "idle", lastActivity: "2026-08-11T01:00:00.000Z",
      }),
    ], null);
    expect(groups.map(g => g.projectPath)).toEqual(["/p/quiet", OTHER_PROJECT]);
    expect(groups[1].counts.waiting).toBe(1);
  });

  it("sorts last even when it is the most recently active", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { lastActivity: "2026-08-11T23:00:00.000Z" }),
      session("claude", "b", {
        projectRoot: "/p/one", lastActivity: "2026-08-11T02:00:00.000Z",
      }),
    ], null);
    expect(groups.map(g => g.projectPath)).toEqual(["/p/one", OTHER_PROJECT]);
  });

  it("sorts last without disturbing how the real projects order among themselves", () => {
    const groups = buildSidebarGroups([
      session("claude", "other", { status: "waiting", lastActivity: "2026-08-11T23:00:00.000Z" }),
      session("claude", "quiet", {
        projectRoot: "/p/quiet", status: "idle", lastActivity: "2026-08-11T22:00:00.000Z",
      }),
      session("claude", "busy", {
        projectRoot: "/p/busy", status: "working", lastActivity: "2026-08-11T03:00:00.000Z",
      }),
      session("claude", "blocked", {
        projectRoot: "/p/blocked", status: "waiting", lastActivity: "2026-08-11T01:00:00.000Z",
      }),
    ], null);
    expect(groups.map(g => g.projectPath))
      .toEqual(["/p/blocked", "/p/busy", "/p/quiet", OTHER_PROJECT]);
  });
});

describe("buildSidebarGroups — counts", () => {
  it("counts total, waiting and working per group", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/p/one", status: "waiting" }),
      session("claude", "b", { projectRoot: "/p/one", status: "working" }),
      session("claude", "c", { projectRoot: "/p/one", status: "working" }),
      session("claude", "d", { projectRoot: "/p/one", status: "idle" }),
      session("claude", "e", { projectRoot: "/p/one", status: "failed" }),
      session("claude", "f", { projectRoot: "/p/two", status: "idle" }),
    ], null);
    const one = groups.find(g => g.projectPath === "/p/one");
    expect(one?.counts).toEqual({ total: 5, waiting: 1, working: 2 });
    expect(one?.counts.total).toBe(one?.rows.length);
    expect(groups.find(g => g.projectPath === "/p/two")?.counts)
      .toEqual({ total: 1, waiting: 0, working: 0 });
  });

  it("counts the Other bucket the same way", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { status: "waiting" }),
      session("claude", "b", { status: "idle" }),
    ], null);
    expect(groups[0].counts).toEqual({ total: 2, waiting: 1, working: 0 });
  });
});

describe("buildSidebarGroups — row order", () => {
  it("orders chats in a project by last activity, newest first", () => {
    const groups = buildSidebarGroups([
      session("claude", "old", {
        projectRoot: "/p/one", status: "working", lastActivity: "2026-08-11T09:00:00.000Z",
      }),
      session("claude", "newest", {
        projectRoot: "/p/one", status: "idle", lastActivity: "2026-08-11T23:00:00.000Z",
      }),
      session("claude", "mid", {
        projectRoot: "/p/one", status: "waiting", lastActivity: "2026-08-11T12:00:00.000Z",
      }),
    ], null);
    expect(groups[0].rows.map(r => r.key))
      .toEqual(["claude#newest", "claude#mid", "claude#old"]);
  });

  it("keeps each project's recency sort when projects interleave", () => {
    const groups = buildSidebarGroups([
      session("claude", "a-old", {
        projectRoot: "/p/one", lastActivity: "2026-08-11T09:00:00.000Z",
      }),
      session("claude", "b-new", {
        projectRoot: "/p/two", lastActivity: "2026-08-11T22:00:00.000Z",
      }),
      session("claude", "a-new", {
        projectRoot: "/p/one", lastActivity: "2026-08-11T21:00:00.000Z",
      }),
      session("claude", "b-old", {
        projectRoot: "/p/two", lastActivity: "2026-08-11T08:00:00.000Z",
      }),
    ], null);
    const byPath = new Map(groups.map(g => [g.projectPath, g]));
    expect(byPath.get("/p/one")?.rows.map(r => r.key)).toEqual(["claude#a-new", "claude#a-old"]);
    expect(byPath.get("/p/two")?.rows.map(r => r.key)).toEqual(["claude#b-new", "claude#b-old"]);
  });
});

describe("buildSidebarGroups — group order", () => {
  // A project's slot is fixed by its name: neither a waiting session nor the
  // newest activity moves it. The waiting group here is alphabetically LAST,
  // so urgency-first and recency-first orders both give the wrong answer.
  it("orders projects by name whatever is waiting, working or recent", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", {
        projectRoot: "/p/mid", status: "working", lastActivity: "2026-08-11T23:00:00.000Z",
      }),
      session("claude", "b", {
        projectRoot: "/p/alpha", status: "idle", lastActivity: "2026-08-11T01:00:00.000Z",
      }),
      session("claude", "c", {
        projectRoot: "/p/zulu", status: "waiting", lastActivity: "2026-08-11T22:00:00.000Z",
      }),
    ], null);
    expect(groups.map(g => g.projectPath)).toEqual(["/p/alpha", "/p/mid", "/p/zulu"]);
  });

  it("does not move a project when its newest activity changes", () => {
    const at = (hour: string) => `2026-08-11T${hour}:00:00.000Z`;
    const before = buildSidebarGroups([
      session("claude", "one1", { projectRoot: "/p/one", lastActivity: at("09") }),
      session("claude", "two1", { projectRoot: "/p/two", lastActivity: at("08") }),
    ], null);
    const after = buildSidebarGroups([
      session("claude", "one1", { projectRoot: "/p/one", lastActivity: at("09") }),
      session("claude", "two1", { projectRoot: "/p/two", lastActivity: at("08") }),
      session("claude", "two2", { projectRoot: "/p/two", status: "waiting", lastActivity: at("12") }),
    ], null);
    expect(before.map(g => g.projectPath)).toEqual(["/p/one", "/p/two"]);
    expect(after.map(g => g.projectPath)).toEqual(["/p/one", "/p/two"]);
  });

  it("breaks a shared name on the full path", () => {
    const groups = buildSidebarGroups([
      session("claude", "a", { projectRoot: "/work/b/app" }),
      session("claude", "b", { projectRoot: "/work/a/app" }),
    ], null);
    expect(groups.map(g => g.projectPath)).toEqual(["/work/a/app", "/work/b/app"]);
  });
});

describe("buildSidebarGroups — selection and purity", () => {
  const sessions = [
    session("claude", "a", { projectRoot: "/p/one", status: "waiting" }),
    session("codex", "b", { projectRoot: "/p/two" }),
    session("claude", "c", { projectRoot: "/p/two" }),
  ];

  it("marks exactly one row across all groups", () => {
    const rows = buildSidebarGroups(sessions, "claude#c").flatMap(g => g.rows);
    expect(rows.filter(r => r.selected)).toHaveLength(1);
    expect(rows.find(r => r.selected)?.key).toBe("claude#c");
  });

  it("marks nothing for no selection or an unknown key", () => {
    expect(buildSidebarGroups(sessions, null).flatMap(g => g.rows).some(r => r.selected))
      .toBe(false);
    expect(buildSidebarGroups(sessions, "claude#gone").flatMap(g => g.rows)
      .some(r => r.selected)).toBe(false);
  });

  it("does not mutate the input array or its sessions", () => {
    // Frozen, so an in-place sort or an edited summary throws instead of passing.
    const input: SessionSummary[] = [
      session("claude", "a", {
        projectRoot: "/p/quiet", lastActivity: "2026-08-11T23:00:00.000Z",
      }),
      session("claude", "b", { projectRoot: "/p/blocked", status: "waiting" }),
      session("codex", "c", { projectRoot: "/p/quiet/", status: "working" }),
    ];
    for (const s of input) Object.freeze(s);
    Object.freeze(input);
    const before = JSON.stringify(input);

    const groups = buildSidebarGroups(input, "claude#b");

    expect(JSON.stringify(input)).toBe(before);
    expect(input.map(s => s.sessionId)).toEqual(["a", "b", "c"]);
    expect(groups.map(g => g.projectPath)).toEqual(["/p/blocked", "/p/quiet"]);
  });

  it("returns fresh rows every call", () => {
    const first = buildSidebarGroups(sessions, null);
    const second = buildSidebarGroups(sessions, null);
    expect(first).toEqual(second);
    expect(first[0]).not.toBe(second[0]);
    expect(first[0].rows[0]).not.toBe(second[0].rows[0]);
  });
});

/* ---- the two view filters ------------------------------------------------
   Shared fixtures. Both filters answer the same shape of question and share two
   vetoes, so the helpers below are used by all three view suites. */

const BIG = TRIVIAL_SESSION_BYTES * 10;
const NONE: ReadonlySet<string> = new Set();
const keys = (view: { groups: SidebarGroup[] }): string[] =>
  view.groups.flatMap(g => g.rows.map(r => r.key));

// A session the engine itself titled with nothing but a slash command. Both
// halves matter: `titleFromSummary` is what separates this from a real session
// whose opening line happened to be "/model".
const command = (id: string, title: string, over: Partial<SessionSummary> = {}) =>
  session("claude", id, { title, titleFromSummary: true, size: BIG, ...over });

// Started in Interface — i.e. carrying the engine-side marker.
const app = (id: string, over: Partial<SessionSummary> = {}) =>
  session("claude", id, { origin: "app", size: BIG, ...over });

describe("conversation surface placement", () => {
  it("keeps conversations in Chat, including code-capable sessions, and tasks in Agent", () => {
    expect(sessionSurfaceOf({})).toBe("chat");
    expect(sessionSurfaceOf({ kind: "chat" })).toBe("chat");
    expect(sessionSurfaceOf({ kind: "voice" })).toBe("chat");
    expect(sessionSurfaceOf({ kind: "code" })).toBe("chat");
    expect(sessionSurfaceOf({ kind: "agent" })).toBe("agent");
  });

  it("does not rewrite execution metadata or infer categories from a title or cwd", () => {
    const imported = session("codex", "imported", {
      title: "Refine quarterly economics article", cwd: "/documents/conversation",
    });
    const codingTitle = { ...imported, title: "Fix failing tests", cwd: "/projects/repo" };
    const before = structuredClone(imported);
    expect(sessionSurfaceOf(imported)).toBe("chat");
    expect(sessionSurfaceOf(codingTitle)).toBe("chat");
    expect(sessionSurfaceOf({ ...imported, kind: "code" })).toBe("chat");
    expect(imported).toEqual(before);
    expect(imported.kind).toBeUndefined();
  });

  it("does not mistake app/store origin or coding permissions for a CLI workspace", () => {
    for (const origin of [undefined, "app", "store"] as const) {
      const work = session("codex", "work", {
        origin, kind: "code", cwd: "/projects/repo", settingSources: [], disallowedTools: ["Write"],
      });
      const before = structuredClone(work);
      expect(sessionSurfaceOf(work)).toBe("chat");
      expect(work).toEqual(before);
    }
  });
});

describe("buildSidebarView — terminal project projection", () => {
  it("joins a bound terminal to its chat's canonical project root", () => {
    const owner = session("claude", "owner", {
      projectRoot: "/work/interface",
      cwd: "/worktrees/interface/sidebar",
      origin: "app",
      size: BIG,
    });
    const view = buildSidebarView([owner], "claude#owner", NONE, false, false, "", {
      terminals: [terminal("terminal-a", {
        root: "/worktrees/interface/sidebar",
        cwd: "/worktrees/interface/sidebar/electron",
        sessionKey: "claude#owner",
        launch: { version: 1, profileId: "claude" },
      })],
      projectSessions: [owner],
      selectedTerminalId: "terminal-a",
    });

    expect(view.groups).toHaveLength(1);
    expect(view.groups[0]?.projectPath).toBe("/work/interface");
    expect(view.groups[0]?.rows[0]?.selected).toBe(false);
    expect(view.groups[0]?.terminals).toEqual([expect.objectContaining({
      id: "terminal-a",
      selected: true,
      profileId: "claude",
    })]);
  });

  it("keeps a bound terminal with its rootless owner's Other group", () => {
    const owner = session("claude", "scratch", { cwd: "/tmp/scratch" });
    const view = buildSidebarView([owner], null, NONE, false, false, "", {
      terminals: [terminal("scratch-terminal", {
        root: "/tmp/scratch",
        cwd: "/tmp/scratch",
        sessionKey: "claude#scratch",
      })],
      projectSessions: [owner],
      selectedTerminalId: null,
    });

    expect(view.groups).toHaveLength(1);
    expect(view.groups[0]?.projectPath).toBe(OTHER_PROJECT);
    expect(view.groups[0]?.rows.map(item => item.key)).toEqual(["claude#scratch"]);
    expect(view.groups[0]?.terminals?.map(item => item.id)).toEqual(["scratch-terminal"]);
  });

  it("retains a terminal's remembered project after its owning chat is archived", () => {
    const view = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [terminal("archived-owner-terminal", {
        root: "/private/worktrees/sidebar-fix",
        cwd: "/private/worktrees/sidebar-fix/electron",
        sessionKey: "claude#archived-owner",
      })],
      projectSessions: [],
      rememberedProjectRoots: new Map([["claude#archived-owner", "/work/interface"]]),
      selectedTerminalId: null,
    });

    expect(view.groups.map(group => group.projectPath)).toEqual(["/work/interface"]);
    expect(view.groups[0]?.terminals?.map(item => item.id))
      .toEqual(["archived-owner-terminal"]);
  });

  it("uses the longest path-boundary project match for an unbound terminal", () => {
    const parent = session("claude", "parent", { projectRoot: "/work/app" });
    const sibling = session("codex", "sibling", { projectRoot: "/work/app-tools" });
    const view = buildSidebarView([parent, sibling], null, NONE, false, false, "", {
      terminals: [
        terminal("nested", { root: "/work/app/electron", cwd: "/work/app/electron" }),
        terminal("prefix-only", {
          root: "/work/application",
          cwd: "/work/application",
        }),
      ],
      projectSessions: [parent, sibling],
      selectedTerminalId: null,
    });

    expect(view.groups.find(group => group.projectPath === "/work/app")?.terminals
      ?.map(item => item.id)).toEqual(["nested"]);
    expect(view.groups.find(group => group.projectPath === "/work/application")?.terminals
      ?.map(item => item.id)).toEqual(["prefix-only"]);
    expect(view.projectCount).toBe(3);
  });

  it("does not file every absolute path under a filesystem-root project", () => {
    // "/" is a prefix of every Unix path. Treating it as an ancestor put every
    // unbound terminal — and then every saved workspace — in one "/" group
    // until a click rebound it through a chat's real projectRoot.
    const rootOwner = session("claude", "root", { projectRoot: "/" });
    const view = buildSidebarView([rootOwner], null, NONE, false, false, "", {
      terminals: [terminal("root-terminal", {
        root: "/private/nested",
        cwd: "/private/nested",
      })],
      projectSessions: [rootOwner],
      selectedTerminalId: null,
    });

    expect(view.groups.map(group => group.projectPath).sort())
      .toEqual(["/", "/private/nested"]);
    expect(view.groups.find(group => group.projectPath === "/")?.terminals)
      .toBeUndefined();
    expect(view.groups.find(group => group.projectPath === "/private/nested")
      ?.terminals?.map(item => item.id)).toEqual(["root-terminal"]);
  });

  it("groups a terminal whose authorized root is exactly the filesystem root", () => {
    const rootOwner = session("claude", "root", { projectRoot: "/" });
    const view = buildSidebarView([rootOwner], null, NONE, false, false, "", {
      terminals: [terminal("root-terminal", { root: "/", cwd: "/" })],
      projectSessions: [rootOwner],
      selectedTerminalId: null,
    });

    expect(view.groups).toHaveLength(1);
    expect(view.groups[0]?.projectPath).toBe("/");
    expect(view.groups[0]?.terminals?.map(item => item.id)).toEqual(["root-terminal"]);
  });

  it("uses a terminal's cwd when its authorized root is the filesystem root", () => {
    const view = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [terminal("poisoned-root", {
        root: "/",
        cwd: "/work/interface",
      })],
      projectSessions: [],
      selectedTerminalId: null,
    });

    expect(view.groups.map(group => group.projectPath)).toEqual(["/work/interface"]);
    expect(view.terminalProjects).toEqual({ "poisoned-root": "/work/interface" });
  });

  it("uses the unfiltered session registry to place a hidden owner's terminal", () => {
    const hiddenOwner = session("claude", "worker", { projectRoot: "/work/orchestrator" });
    const view = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [terminal("worker-terminal", {
        root: "/private/worktree/worker",
        cwd: "/private/worktree/worker",
        sessionKey: "claude#worker",
      })],
      projectSessions: [hiddenOwner],
      selectedTerminalId: null,
    });

    expect(view.groups.map(group => group.projectPath)).toEqual(["/work/orchestrator"]);
    expect(view.groups[0]?.rows).toEqual([]);
    expect(view.groups[0]?.terminals?.map(item => item.id)).toEqual(["worker-terminal"]);
    expect(view.projectCount).toBe(1);
  });

  it("searches terminal facts while keeping the unfiltered project count stable", () => {
    const owner = session("claude", "owner", { projectRoot: "/work/interface" });
    const view = buildSidebarView([owner], null, NONE, false, false, "codex running", {
      terminals: [
        terminal("codex-terminal", {
          root: "/work/interface",
          cwd: "/work/interface",
          launch: { version: 1, profileId: "codex" },
        }),
        terminal("shell-terminal", {
          root: "/work/standalone",
          cwd: "/work/standalone",
          state: "exited",
        }),
      ],
      projectSessions: [owner],
      selectedTerminalId: null,
    });

    expect(view.groups).toHaveLength(1);
    expect(view.groups[0]?.projectPath).toBe("/work/interface");
    expect(view.groups[0]?.rows).toEqual([]);
    expect(view.groups[0]?.terminals?.map(item => item.id)).toEqual(["codex-terminal"]);
    expect(view.projectCount).toBe(2);
    expect(view.searching).toBe(true);
  });

  it("reports busy terminal projects from the full registry, not idle processes or search matches", () => {
    const owner = session("claude", "owner", { projectRoot: "/work/interface" });
    const view = buildSidebarView([owner], null, NONE, false, false, "build exited", {
      terminals: [
        terminal("hidden-running", {
          title: "Claude Code",
          root: "/work/idle",
          cwd: "/work/idle",
          activity: "idle",
        }),
        terminal("hidden-working", {
          title: "Codex",
          root: "/work/interface",
          cwd: "/work/interface",
          activity: "working",
        }),
        terminal("visible-exited", {
          title: "Build shell",
          root: "/work/interface",
          cwd: "/work/interface",
          state: "exited",
        }),
      ],
      projectSessions: [owner],
      selectedTerminalId: null,
    });

    expect(view.groups[0]?.terminals?.map(item => item.id)).toEqual(["visible-exited"]);
    expect(view.protectedProjects).toEqual(["/work/interface"]);
    expect(view.terminalProjects).toEqual({
      "hidden-running": "/work/idle",
      "hidden-working": "/work/interface",
      "visible-exited": "/work/interface",
    });
  });

  it("protects working agents from the full registry when Code hides chat rows", () => {
    const worker = session("claude", "worker", {
      projectRoot: "/work/interface",
      status: "working",
    });
    const view = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [],
      registryTerminals: [],
      projectSessions: [worker],
      selectedTerminalId: null,
    });

    expect(view.groups).toEqual([]);
    expect(view.protectedProjects).toEqual(["/work/interface"]);
  });

  it("protects busy terminals from the full registry when Chat hides terminal rows", () => {
    const owner = session("claude", "owner", {
      projectRoot: "/work/interface",
      status: "idle",
    });
    const view = buildSidebarView([owner], null, NONE, false, false, "", {
      terminals: [],
      registryTerminals: [
        terminal("busy", {
          root: "/work/interface",
          activity: "waiting",
        }),
        terminal("idle", {
          root: "/work/quiet",
          activity: "idle",
        }),
      ],
      projectSessions: [owner],
      selectedTerminalId: null,
    });

    expect(view.groups[0]?.terminals).toBeUndefined();
    expect(view.protectedProjects).toEqual(["/work/interface"]);
    expect(view.terminalProjects).toEqual({
      busy: "/work/interface",
      idle: "/work/quiet",
    });
  });

  it("leaves an idle agent and idle terminal archivable on both surfaces", () => {
    const quiet = session("claude", "quiet", {
      projectRoot: "/work/interface",
      status: "idle",
    });
    const shell = terminal("shell", {
      root: "/work/interface",
      activity: "idle",
    });
    const code = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [shell],
      registryTerminals: [shell],
      projectSessions: [quiet],
      selectedTerminalId: null,
    });
    const chat = buildSidebarView([quiet], null, NONE, false, false, "", {
      terminals: [],
      registryTerminals: [shell],
      projectSessions: [quiet],
      selectedTerminalId: null,
    });

    expect(code.protectedProjects).toEqual([]);
    expect(code.projectCount).toBe(1);
    expect(chat.protectedProjects).toEqual([]);
    expect(chat.projectCount).toBe(1);
  });

  it("treats a persistent idle shell as archivable but protects actual terminal work", () => {
    expect(terminalHasActiveWork({ state: "running", activity: "idle" })).toBe(false);
    expect(terminalHasActiveWork({ state: "starting", activity: "idle" })).toBe(false);
    expect(terminalHasActiveWork({ state: "starting", activity: "waiting" })).toBe(true);
    expect(terminalHasActiveWork({ state: "running", activity: "working" })).toBe(true);
    expect(terminalHasActiveWork({ state: "running", activity: "waiting" })).toBe(true);
    expect(terminalHasActiveWork({ state: "exited", activity: "working" })).toBe(false);
  });

  it("does not project stale live activity after a terminal finishes", () => {
    const view = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [
        terminal("starting", { root: "/work/project", state: "starting", activity: "waiting" }),
        terminal("running", { root: "/work/project", activity: "working" }),
        terminal("exited", { root: "/work/project", state: "exited", activity: "working" }),
        terminal("failed", { root: "/work/project", state: "failed" }),
      ],
      projectSessions: [],
      selectedTerminalId: null,
      activities: { failed: "waiting" },
    });
    const activity = Object.fromEntries((view.groups[0]?.terminals ?? [])
      .map(item => [item.id, item.activity]));

    expect(activity).toEqual({
      starting: "waiting",
      running: "working",
      failed: undefined,
      exited: undefined,
    });
  });

  it("does not lift a finished terminal's project on stale working activity", () => {
    const view = buildSidebarView([], null, NONE, false, false, "", {
      terminals: [
        terminal("stale", {
          root: "/work/stale",
          state: "exited",
          activity: "working",
          updatedAt: "2026-08-11T09:00:00.000Z",
        }),
        terminal("quiet", {
          root: "/work/quiet",
          updatedAt: "2026-08-11T10:00:00.000Z",
        }),
      ],
      projectSessions: [],
      selectedTerminalId: null,
    });

    expect(view.groups.map(group => group.projectPath))
      .toEqual(["/work/quiet", "/work/stale"]);
  });
});

describe("workspaceSidebarBadge", () => {
  it("ranks failed, then a request for input, then live work, then rest", () => {
    expect(workspaceSidebarBadge([
      { state: "failed" }, { state: "running", activity: "waiting" },
    ])).toBe("failed");
    expect(workspaceSidebarBadge([
      { state: "running", activity: "waiting" },
      { state: "running", activity: "working" },
    ])).toBe("waiting");
    expect(workspaceSidebarBadge([{ state: "starting" }])).toBe("idle");
    expect(workspaceSidebarBadge([{ state: "running", activity: "working" }])).toBe("working");
    expect(workspaceSidebarBadge([{ state: "running" }])).toBe("idle");
    expect(workspaceSidebarBadge([{ state: "exited", activity: "waiting" }])).toBe("idle");
    expect(workspaceSidebarBadge([{ state: "exited", activity: "working" }])).toBe("idle");
    expect(workspaceSidebarBadge([])).toBe("idle");
  });
});

describe("workspaceSidebarDone", () => {
  it("shows blue only on a resting workspace whose live agent finished unseen", () => {
    expect(workspaceSidebarDone([{ state: "running", done: true }, { state: "running" }])).toBe(true);
    expect(workspaceSidebarDone([{ state: "running" }])).toBe(false);
    // Every other badge outranks done.
    expect(workspaceSidebarDone([
      { state: "running", done: true }, { state: "running", activity: "working" },
    ])).toBe(false);
    expect(workspaceSidebarDone([
      { state: "running", done: true }, { state: "running", activity: "waiting" },
    ])).toBe(false);
    expect(workspaceSidebarDone([{ state: "running", done: true }, { state: "failed" }])).toBe(false);
    // An exited process is not a finished turn anyone still has to look at.
    expect(workspaceSidebarDone([{ state: "exited", done: true }])).toBe(false);
    expect(workspaceSidebarDone([])).toBe(false);
  });
});

describe("workspaceSidebarPips", () => {
  it("draws one light per terminal, in order, with the pane's own precedence", () => {
    expect(workspaceSidebarPips([
      { state: "running", activity: "working" },
      { state: "running", activity: "waiting", done: true },
      { state: "running", done: true },
      { state: "failed" },
      { state: "running" },
      { state: "starting" },
    ]).shown.map(pip => pip.light)).toEqual(["working", "waiting", "done", "failed", "idle", "idle"]);
  });

  it("names each agent pane's provider so Codex and Claude pips differ", () => {
    expect(workspaceSidebarPips([
      { state: "running", activity: "working", profileId: "codex" },
      { state: "running", activity: "working", profileId: "claude" },
      { state: "running", profileId: "codex" },
      { state: "running", activity: "working", profileId: "shell" },
      { state: "running", activity: "working" },
    ]).shown).toEqual([
      { light: "working", profileId: "codex" },
      { light: "working", profileId: "claude" },
      { light: "idle", profileId: "codex" },
      { light: "working" },
      { light: "working" },
    ]);
  });

  it("drops exited terminals and caps the row with a count", () => {
    expect(workspaceSidebarPips([{ state: "exited", done: true }]).shown).toEqual([]);
    const many = workspaceSidebarPips(Array.from({ length: 11 }, () => ({ state: "running" as const })));
    expect(many.shown).toHaveLength(8);
    expect(many.more).toBe(3);
    // Past the cap, a pane that finished or needs you is never folded away.
    const late = workspaceSidebarPips([
      ...Array.from({ length: 10 }, () => ({ state: "running" as const })),
      { state: "running" as const, done: true },
      { state: "running" as const, activity: "waiting" as const },
    ]);
    expect(late.shown.slice(0, 2).map(pip => pip.light)).toEqual(["done", "waiting"]);
    expect(late.shown).toHaveLength(8);
    expect(late.more).toBe(4);
  });
});

describe("workspacesActiveFirst", () => {
  const terminal = (id: string, activity?: "working" | "waiting", state: TerminalSummary["state"] = "running") => ({
    id, title: id, state, root: "/p", cwd: "/p", lastActivity: "", selected: false,
    ...(activity === undefined ? {} : { activity }),
  });
  const record = (id: string, ...terminalIds: string[]) => ({ id, terminalIds });

  it("lifts workspaces with live work above the rest and keeps the saved order otherwise", () => {
    const ordered = workspacesActiveFirst(
      [record("quiet", "q"), record("dead", "d"), record("busy", "b"), record("ask", "a"),
        record("empty")],
      [terminal("q"), terminal("d", undefined, "failed"), terminal("b", "working"),
        terminal("a", "waiting")],
    );
    expect(ordered.map(r => r.id)).toEqual(["busy", "ask", "quiet", "dead", "empty"]);
  });

  it("does not mutate the saved roster", () => {
    const records = Object.freeze([record("quiet", "q"), record("busy", "b")]);
    expect(workspacesActiveFirst(records, [terminal("b", "working")]).map(r => r.id))
      .toEqual(["busy", "quiet"]);
    expect(records.map(r => r.id)).toEqual(["quiet", "busy"]);
  });
});

describe("workspaceSidebarTitle", () => {
  it("keeps a workspace identity when focus and activity change", () => {
    const members = [
      { title: "Fix resume", selected: false, lastActivity: "2026-09-09" },
      { title: "Unrelated task", selected: true, lastActivity: "2026-09-10" },
    ];
    expect(workspaceSidebarTitle("Workspace 3", members, "Fix resume")).toBe("Fix resume");
    expect(workspaceSidebarTitle("Workspace 3", members.map(m => ({ ...m, selected: !m.selected })), "Fix resume")).toBe("Fix resume");
    expect(workspaceSidebarTitle("Workspace 3", [], "Fix resume")).toBe("Fix resume");
    expect(workspaceSidebarTitle("Workspace 3", [members[1]], "Fix resume")).toBe("Fix resume");
    expect(workspaceSidebarTitle("Release review", members, "Unrelated task")).toBe("Release review");
    expect(workspaceSidebarTitle("Workspace 3", [])).toBe("Workspace 3");
    expect(workspaceSidebarTitle("Please keep this exact name", members)).toBe("Please keep this exact name");
  });
});

describe("workspaceSidebarProjectPath", () => {
  it("keeps a saved workspace visible from its cwd when PTYs have not reopened", () => {
    expect(workspaceSidebarProjectPath(
      ["gone"], {}, "/work/interface", ["/work/interface"],
    )).toBe("/work/interface");
    expect(workspaceSidebarProjectPath(
      ["gone"], {}, "/work/interface/electron", ["/work/interface"],
    )).toBe("/work/interface");
    expect(workspaceSidebarProjectPath(
      ["live"], { live: "/work/other" }, "/work/interface", ["/work/interface"],
    )).toBe("/work/other");
    expect(workspaceSidebarProjectPath(["gone"], {}, "", [])).toBeUndefined();
  });

  it("does not file every saved cwd under a filesystem-root project", () => {
    expect(workspaceSidebarProjectPath(
      ["gone"], {}, "/work/interface", ["/"],
    )).toBe("/work/interface");
    expect(workspaceSidebarProjectPath(
      ["gone"], {}, "/work/other", ["/", "/work/interface"],
    )).toBe("/work/other");
  });

  it("ignores a live filesystem-root assignment when the saved cwd is more specific", () => {
    expect(workspaceSidebarProjectPath(
      ["live"], { live: "/" }, "/work/interface", ["/"],
    )).toBe("/work/interface");
    expect(workspaceSidebarProjectPath(
      ["live"], { live: "/" }, "/", ["/"],
    )).toBe("/");
  });
});

describe("buildSidebarView — the declutter filter", () => {
  describe("the threshold constant", () => {
    it("sits inside the measured plateau", () => {
      // The size study found every threshold in [24029 … 28304] hides the exact
      // same 13 sessions. A value outside that range is a behaviour change on
      // real data and should fail here first.
      expect(TRIVIAL_SESSION_BYTES).toBeGreaterThanOrEqual(24_029);
      expect(TRIVIAL_SESSION_BYTES).toBeLessThanOrEqual(28_304);
    });

    it("is a whole number of KiB, so the label the user reads is exact", () => {
      // The tooltip renders `${TRIVIAL_SESSION_KIB} KiB` with no rounding step.
      // Move the constant off a 1024 boundary and the sentence would silently
      // acquire a fraction (or, with the old Math.round, silently lie).
      expect(Number.isInteger(TRIVIAL_SESSION_KIB)).toBe(true);
      expect(TRIVIAL_SESSION_KIB * 1024).toBe(TRIVIAL_SESSION_BYTES);
    });
  });

  describe("(a) status — only idle is ever hideable", () => {
    // The previous version hid a session that was blocked on the user's
    // approval, and hid crashed launches systematically: a failed launch writes
    // a tiny transcript, so a size-only rule removes exactly the sessions the
    // user most needs to see.
    const notIdle: SessionStatus[] = ["waiting", "working", "failed"];

    it("never hides a non-idle session, however small", () => {
      for (const status of notIdle) {
        const view = buildSidebarView(
          [session("claude", "s", { status, size: 100 })], null, NONE, true, false);
        expect(keys(view)).toEqual(["claude#s"]);
        expect(view.hidden.trivial).toBe(0);
      }
    });

    it("never hides a non-idle session whose title is a bare slash command", () => {
      for (const status of notIdle) {
        const view = buildSidebarView(
          [command("s", "/doctor", { status })], null, NONE, true, false);
        expect(keys(view)).toEqual(["claude#s"]);
        expect(view.hidden.trivial).toBe(0);
      }
    });

    it("hides the same session once it goes idle", () => {
      // The mirror of the two above: without this, a test suite that only ever
      // asserted "not hidden" would pass with the filter disabled entirely.
      const view = buildSidebarView(
        [command("s", "/doctor", { status: "idle" })], null, NONE, true, false);
      expect(keys(view)).toEqual([]);
      expect(view.hidden.trivial).toBe(1);
    });
  });

  describe("(c1) a bare slash-command title from the engine's own summary", () => {
    it("hides /doctor even though its transcript is far above the size cut", () => {
      // The user's actual complaint, and the case a byte rule can never reach:
      // the three real /doctor transcripts are 287/326/334 KB.
      const view = buildSidebarView([
        command("doctor", "/doctor", { size: 334_000 }),
        session("claude", "real", { title: "Refactor the parser", size: BIG }),
      ], null, NONE, true, false);
      expect(keys(view)).toEqual(["claude#real"]);
      expect(view.hidden.trivial).toBe(1);
    });

    it("hides every bare command form seen in the real corpus", () => {
      // /usage x7, /model x3, /doctor x3, /effort, /config, /mcp, /cd — 17 of 87.
      const titles = ["/usage", "/model", "/doctor", "/effort", "/config", "/mcp", "/cd"];
      const view = buildSidebarView(
        titles.map((t, i) => command(`c${i}`, t)), null, NONE, true, false);
      expect(keys(view)).toEqual([]);
      expect(view.hidden.trivial).toBe(titles.length);
    });

    it("matches case-insensitively and allows digits and dashes", () => {
      const titles = ["/Doctor", "/IDE", "/add-dir", "/x2"];
      const view = buildSidebarView(
        titles.map((t, i) => command(`c${i}`, t)), null, NONE, true, false);
      expect(view.hidden.trivial).toBe(titles.length);
    });

    it("never hides a title that merely STARTS with a slash command", () => {
      // The anchor at the end of the pattern is the whole safety margin here.
      const titles = [
        "/model please switch to opus",
        "/doctor output was confusing",
        "/usage — and then fix the bill",
      ];
      const view = buildSidebarView(
        titles.map((t, i) => command(`c${i}`, t)), null, NONE, true, false);
      expect(keys(view)).toEqual(["claude#c0", "claude#c1", "claude#c2"]);
      expect(view.hidden.trivial).toBe(0);
    });

    it("never hides near-miss shapes that are not slash commands", () => {
      const titles = ["/", "//", "/9lives", "/-dash", "model", "", "/a b"];
      const view = buildSidebarView(
        titles.map((t, i) => command(`c${i}`, t)), null, NONE, true, false);
      expect(view.hidden.trivial).toBe(0);
    });

    it("never hides a slash-command title that came from a firstPrompt fallback", () => {
      // THE load-bearing distinction. A real session that opened with "/model"
      // and has not been summarised yet carries the raw prompt as its title;
      // the engine has passed no verdict on it, so neither may this filter.
      const view = buildSidebarView([
        session("claude", "raw", { title: "/model", titleFromSummary: false, size: BIG }),
        session("claude", "unknown", { title: "/model", size: BIG }),   // flag absent
        command("summarised", "/model"),
      ], null, NONE, true, false);
      expect(keys(view)).toEqual(["claude#raw", "claude#unknown"]);
      expect(view.hidden.trivial).toBe(1);
    });

    it("hides a slash command whose size the engine never reported", () => {
      // The title branch stands on its own — it does not need a size, which is
      // what lets it work for an engine that reports none.
      const view = buildSidebarView(
        [command("c", "/usage", { size: undefined })], null, NONE, true, false);
      expect(keys(view)).toEqual([]);
      expect(view.hidden.trivial).toBe(1);
    });
  });

  describe("(c2) size, the secondary signal", () => {
    it("hides a small idle session whose title is ordinary prose", () => {
      // "Switch to Opus model" at 2,523 bytes: the one session in the corpus
      // the title rule misses and this one catches.
      const view = buildSidebarView([
        session("claude", "small", { title: "Switch to Opus model", size: 2_523 }),
        session("claude", "real", { title: "Refactor the parser", size: BIG }),
      ], null, NONE, true, false);
      expect(keys(view)).toEqual(["claude#real"]);
      expect(view.hidden.trivial).toBe(1);
    });

    it("keeps a session exactly at the threshold — the cut is strictly below", () => {
      const view = buildSidebarView([
        session("claude", "at", { size: TRIVIAL_SESSION_BYTES }),
        session("claude", "under", { size: TRIVIAL_SESSION_BYTES - 1 }),
        session("claude", "over", { size: TRIVIAL_SESSION_BYTES + 1 }),
      ], null, NONE, true, false);
      expect(keys(view)).toEqual(["claude#at", "claude#over"]);
      expect(view.hidden.trivial).toBe(1);
    });

    it("never treats an unreported size as small", () => {
      // Every Codex thread is in this bucket today. Absent is the engine
      // declining to answer, not evidence of smallness.
      const view = buildSidebarView([
        session("codex", "unknown"),
        session("claude", "small", { size: 100 }),
      ], null, NONE, true, false);
      expect(keys(view)).toEqual(["codex#unknown"]);
      expect(view.hidden.trivial).toBe(1);
    });
  });

  describe("(b) revealed — sticky for the whole app run", () => {
    const chatter = [
      session("claude", "a", { projectRoot: "/p/chatter", size: 900 }),
      session("claude", "b", { projectRoot: "/p/chatter", size: 900 }),
      session("claude", "c", { projectRoot: "/p/work", size: 900 }),
    ];

    it("never hides a revealed session, and never counts it", () => {
      const view = buildSidebarView(chatter, null, new Set(["claude#a"]), true, false);
      expect(keys(view)).toEqual(["claude#a"]);
      expect(view.hidden.trivial).toBe(2);
    });

    it("keeps a revealed row visible after the selection moves away from it", () => {
      // D2. The old rule exempted only the CURRENT selection, so this row
      // vanished on the click that selected another one.
      const revealed = new Set(["claude#a", "claude#c"]);
      const view = buildSidebarView(chatter, "claude#c", revealed, true, false);
      expect(keys(view)).toEqual(["claude#a", "claude#c"]);
      expect(view.groups.map(g => g.projectPath)).toEqual(["/p/chatter", "/p/work"]);
    });

    it("keeps the project header alive when its last row is only revealed", () => {
      // The compounding half of D2: /p/chatter's whole group used to disappear
      // mid-click, header and all.
      const view = buildSidebarView(
        chatter, "claude#c", new Set(["claude#a", "claude#c"]), true, false);
      const group = view.groups.find(g => g.projectPath === "/p/chatter");
      expect(group?.rows.map(r => r.key)).toEqual(["claude#a"]);
      expect(group?.counts.total).toBe(1);
    });

    it("exempts the current selection even when the caller has not added it yet", () => {
      const view = buildSidebarView(chatter, "claude#b", NONE, true, false);
      expect(keys(view)).toEqual(["claude#b"]);
      expect(view.hidden.trivial).toBe(2);
    });

    it("does not mutate the caller's revealed set", () => {
      // The exemption above has to fold the current selection in WITHOUT
      // writing to React state the caller owns. Object.freeze would not catch
      // this — it does not seal a Set's internal slots — so the membership is
      // checked directly.
      const revealed = new Set(["claude#a"]);
      buildSidebarView(chatter, "claude#b", revealed, true, false);
      expect([...revealed]).toEqual(["claude#a"]);
    });
  });

  describe("counts, groups and the filter toggle", () => {
    it("shows everything when the filter is off, and still counts what it would hide", () => {
      // Started in Interface, so the origin rule has nothing to say about any of
      // them and `hideable` is purely this filter's number.
      const sessions = [
        session("claude", "small", { size: 500, origin: "app" }),
        command("cmd", "/usage", { origin: "app" }),
        session("claude", "real", { size: BIG, origin: "app" }),
        session("codex", "unknown", { origin: "app" }),
      ];
      const off = buildSidebarView(sessions, null, NONE, false, false);
      expect(keys(off))
        .toEqual(["claude#small", "claude#cmd", "claude#real", "codex#unknown"]);
      expect(off.groups).toEqual(buildSidebarGroups(sessions, null));
      expect(off.hidden).toEqual({ external: 0, trivial: 0 });
      // Without this the filter row would have nothing to offer once it is off.
      expect(off.hideable).toBe(2);
    });

    it("counts nothing when no session is hideable", () => {
      const view = buildSidebarView([
        session("claude", "a", { size: BIG, origin: "app" }),
        session("codex", "b", { origin: "app" }),
      ], null, NONE, true, false);
      expect(view.hidden).toEqual({ external: 0, trivial: 0 });
      expect(view.hideable).toBe(0);
      expect(keys(view)).toEqual(["claude#a", "codex#b"]);
    });

    it("makes group counts honest — a header never claims a hidden row", () => {
      const view = buildSidebarView([
        session("claude", "a", { projectRoot: "/p/one", status: "waiting", size: 200 }),
        command("b", "/usage", { projectRoot: "/p/one" }),
        session("claude", "c", { projectRoot: "/p/one", status: "working", size: BIG }),
        session("claude", "d", { projectRoot: "/p/one", size: BIG }),
      ], null, NONE, true, false);
      const one = view.groups.find(g => g.projectPath === "/p/one");
      // The waiting row survives on status alone, despite being 200 bytes.
      expect(one?.rows.map(r => r.key)).toEqual(["claude#a", "claude#c", "claude#d"]);
      expect(one?.counts).toEqual({ total: 3, waiting: 1, working: 1 });
      expect(one?.counts.total).toBe(one?.rows.length);
      expect(view.hidden.trivial).toBe(1);
    });

    it("drops a group whose every session is hidden", () => {
      const view = buildSidebarView([
        command("a", "/usage", { projectRoot: "/p/chatter" }),
        session("claude", "b", { projectRoot: "/p/chatter", size: 900 }),
        session("claude", "c", { projectRoot: "/p/work", size: BIG }),
      ], null, NONE, true, false);
      expect(view.groups.map(g => g.projectPath)).toEqual(["/p/work"]);
      expect(view.hidden.trivial).toBe(2);
    });

    it("leaves row and group ordering to buildSidebarGroups", () => {
      const view = buildSidebarView([
        command("hidden", "/usage", {
          projectRoot: "/p/quiet", lastActivity: "2026-08-11T23:00:00.000Z",
        }),
        session("claude", "quiet1", {
          projectRoot: "/p/quiet", size: BIG, lastActivity: "2026-08-11T09:00:00.000Z",
        }),
        session("claude", "blocked1", {
          projectRoot: "/p/blocked", status: "waiting", size: BIG,
          lastActivity: "2026-08-11T01:00:00.000Z",
        }),
      ], null, NONE, true, false);
      expect(view.groups.map(g => g.projectPath)).toEqual(["/p/blocked", "/p/quiet"]);
      expect(keys(view)).toEqual(["claude#blocked1", "claude#quiet1"]);
    });
  });
});

describe("buildSidebarView — the origin filter", () => {
  it("lists only the chats started in Interface", () => {
    const view = buildSidebarView([
      app("mine"),
      session("claude", "cli", { origin: "store", size: BIG }),
    ], null, NONE, false, true);
    expect(keys(view)).toEqual(["claude#mine"]);
    expect(view.hidden).toEqual({ external: 1, trivial: 0 });
  });

  it("treats an ABSENT origin as store, never as app", () => {
    // The direction is the whole point: no session on this machine carries the
    // marker yet, and guessing "app" for unknown would put every terminal
    // session straight back into the sidebar the user asked to keep clean.
    const view = buildSidebarView([
      session("claude", "unknown", { size: BIG }),
      app("mine"),
    ], null, NONE, false, true);
    expect(keys(view)).toEqual(["claude#mine"]);
    expect(view.hidden.external).toBe(1);
  });

  it("empties the sidebar on a first run where nothing is tagged", () => {
    // The consequence that has to be got right: ~112 rows become 0, and the
    // count the zero-state quotes comes from here.
    const untagged = Array.from({ length: 112 },
      (_, i) => session("claude", `s${i}`, { size: BIG, projectRoot: `/p/${i % 8}` }));
    const view = buildSidebarView(untagged, null, NONE, false, true);
    expect(view.groups).toEqual([]);
    expect(view.hidden).toEqual({ external: 112, trivial: 0 });
    expect(view.hideable).toBe(112);
    // Grouped/flat mode is decided from the UNFILTERED listing, so emptying the
    // pane does not also re-render it as a flat list.
    expect(view.projectCount).toBe(8);
  });

  it("shows everything when the filter is off, and still counts what it would hide", () => {
    const sessions = [app("mine"), session("claude", "cli", { size: BIG })];
    const view = buildSidebarView(sessions, null, NONE, false, false);
    expect(keys(view)).toEqual(["claude#mine", "claude#cli"]);
    expect(view.hidden).toEqual({ external: 0, trivial: 0 });
    expect(view.hideable).toBe(1);
  });

  it("never hides a session that is waiting, working or failed", () => {
    // The defect this reintroduces if it regresses: a terminal session blocked
    // on the user's approval, invisible in the app that is meant to surface it.
    for (const status of ["waiting", "working", "failed"] as SessionStatus[]) {
      const view = buildSidebarView(
        [session("claude", "s", { status, size: BIG })], null, NONE, false, true);
      expect(keys(view)).toEqual(["claude#s"]);
      expect(view.hidden.external).toBe(0);
      expect(view.hideable).toBe(0);
    }
  });

  it("hides the same session once it goes idle", () => {
    // The mirror of the rule above: without it the status veto could be widened
    // to "never hide anything" and every test would still pass.
    const view = buildSidebarView(
      [session("claude", "s", { status: "idle", size: BIG })], null, NONE, false, true);
    expect(keys(view)).toEqual([]);
    expect(view.hidden.external).toBe(1);
  });

  it("never hides the selected session, even before the caller reveals it", () => {
    const view = buildSidebarView([
      session("claude", "cli", { size: BIG }),
      session("claude", "other", { size: BIG }),
    ], "claude#cli", NONE, false, true);
    expect(keys(view)).toEqual(["claude#cli"]);
    expect(view.hidden.external).toBe(1);
  });

  it("never hides a revealed session, and keeps it after the selection moves", () => {
    const sessions = [
      session("claude", "a", { size: BIG }),
      session("claude", "b", { size: BIG }),
      session("claude", "c", { size: BIG }),
    ];
    const view = buildSidebarView(
      sessions, "claude#c", new Set(["claude#a", "claude#c"]), false, true);
    expect(keys(view)).toEqual(["claude#a", "claude#c"]);
    expect(view.hidden.external).toBe(1);
  });

  it("groups what survives exactly as it groups anything else", () => {
    const view = buildSidebarView([
      app("a", { projectRoot: "/work/interface" }),
      app("b", { cwd: "/work/interface/src", projectRoot: "/work/interface" }),
      app("c"),
      session("claude", "cli", { projectRoot: "/work/interface", size: BIG }),
    ], null, NONE, false, true);
    expect(view.groups.map(g => g.projectPath)).toEqual(["/work/interface", OTHER_PROJECT]);
    expect(view.groups[0].rows.map(r => r.key)).toEqual(["claude#a", "claude#b"]);
    expect(view.groups[0].counts.total).toBe(2);
    expect(view.groups[1].rows.map(r => r.key)).toEqual(["claude#c"]);
  });

  it("is a view filter: it never touches the listing it was given", () => {
    // It must not look like a mutation either — nothing here writes, clears or
    // reorders an origin, which is an engine-side tag.
    const input: SessionSummary[] = [
      session("claude", "cli", { size: BIG }),
      app("mine"),
    ];
    for (const s of input) Object.freeze(s);
    Object.freeze(input);
    const before = JSON.stringify(input);

    buildSidebarView(input, null, NONE, false, true);
    buildSidebarView(input, null, NONE, true, true);

    expect(JSON.stringify(input)).toBe(before);
    expect(input.map(s => s.origin)).toEqual([undefined, "app"]);
  });
});

describe("buildSidebarView — importable, the number the import buttons key off", () => {
  // WHY THIS IS NOT `hidden.external`. That count is a property of the FILTER:
  // it is zero the moment the origin toggle is off, and it excludes every
  // session the filters never touch. Keyed to it, the one bulk-import control
  // in the app vanished exactly when the user clicked "Show all" to look at
  // those sessions before importing them.
  const mixed: SessionSummary[] = [
    app("mine"),
    session("claude", "cli", { size: BIG }),
    session("codex", "thread", { origin: "store", size: BIG }),
  ];

  it("counts the sessions that are not Interface's, whatever the filters are doing", () => {
    for (const [hideTrivial, appOnly] of [[true, true], [false, false], [true, false]]) {
      const view = buildSidebarView(mixed, null, NONE, hideTrivial, appOnly);
      expect(view.importable).toBe(2);
    }
  });

  it("stays put when the filter is switched off and hidden.external collapses to 0", () => {
    // THE DEFECT, in one assertion: same listing, filter off, and the number
    // the buttons read must not move with it.
    const on = buildSidebarView(mixed, null, NONE, false, true);
    const off = buildSidebarView(mixed, null, NONE, false, false);
    expect(on.hidden.external).toBe(2);
    expect(off.hidden.external).toBe(0);
    expect(on.importable).toBe(2);
    expect(off.importable).toBe(2);
  });

  it("counts sessions no filter would ever hide — which the import marks anyway", () => {
    // Neither rule touches a non-idle session or one opened this run, so
    // `hidden.external` understates the work by exactly the rows in front of
    // the user. `adoptAll` marks every session in the registry.
    const view = buildSidebarView([
      session("claude", "waiting", { status: "waiting", size: BIG }),
      session("claude", "seen", { size: BIG }),
      session("claude", "quiet", { size: BIG }),
    ], null, new Set(["claude#seen"]), false, true);
    expect(view.hidden.external).toBe(1);
    expect(view.importable).toBe(3);
  });

  it("is 0 once everything carries the marker", () => {
    expect(buildSidebarView([app("a"), app("b")], null, NONE, true, true).importable).toBe(0);
  });

  it("is 0 for an empty listing", () => {
    expect(buildSidebarView([], null, NONE, true, true).importable).toBe(0);
  });

  it("counts an ABSENT origin as importable, the same way the filter does", () => {
    expect(buildSidebarView([session("claude", "untagged", { size: BIG })],
      null, NONE, true, true).importable).toBe(1);
  });

  it("is unaffected by the search, which hides rows without un-importing them", () => {
    const view = buildSidebarView(mixed, null, NONE, false, false, "nothing-matches-this");
    expect(keys(view)).toEqual([]);
    expect(view.importable).toBe(2);
  });
});

describe("buildSidebarView — the two filters together", () => {
  // Every interesting combination in one listing: hideable by neither, by one,
  // by the other, and by both.
  const fixture: SessionSummary[] = [
    app("keep", { size: BIG }),                                   // neither
    session("claude", "cli", { size: BIG }),                       // origin only
    app("cmd", { title: "/usage", titleFromSummary: true }),       // declutter only
    session("claude", "both", { title: "/doctor", titleFromSummary: true }),  // both
    session("claude", "blocked", { status: "waiting", size: 100 }),          // vetoed
  ];

  it("attributes a doubly-hideable session to exactly one rule", () => {
    const view = buildSidebarView(fixture, null, NONE, true, true);
    expect(keys(view)).toEqual(["claude#keep", "claude#blocked"]);
    // "both" is counted under origin, not under both headings — otherwise the
    // row would claim four hidden sessions where three are missing.
    expect(view.hidden).toEqual({ external: 2, trivial: 1 });
  });

  it("counts a doubly-hideable session once in hideable", () => {
    const view = buildSidebarView(fixture, null, NONE, true, true);
    expect(view.hideable).toBe(3);
  });

  it("reports hideable identically whatever the toggles are doing", () => {
    // It is the answer to "what would Hide do", so it cannot depend on what is
    // currently hidden.
    for (const hideTrivial of [true, false]) {
      for (const appOnly of [true, false]) {
        expect(buildSidebarView(fixture, null, NONE, hideTrivial, appOnly).hideable).toBe(3);
      }
    }
  });

  it("hidden always sums to exactly the rows missing from groups", () => {
    // The invariant the sidebar's sentence rests on, checked in all four states
    // — including the mixed ones a stale localStorage value can produce.
    for (const hideTrivial of [true, false]) {
      for (const appOnly of [true, false]) {
        const view = buildSidebarView(fixture, null, NONE, hideTrivial, appOnly);
        const missing = fixture.length - keys(view).length;
        expect(view.hidden.external + view.hidden.trivial).toBe(missing);
        expect(view.hideable).toBeGreaterThanOrEqual(missing);
      }
    }
  });

  it("attributes to the declutter rule alone when only that filter is on", () => {
    // The mixed state a user who turned the old filter off carries into this
    // version: origin off, declutter on. "both" is hidden BY the declutter rule
    // here, and the count has to say so rather than blaming a filter that is off.
    const view = buildSidebarView(fixture, null, NONE, true, false);
    expect(keys(view)).toEqual(["claude#keep", "claude#cli", "claude#blocked"]);
    expect(view.hidden).toEqual({ external: 0, trivial: 2 });
  });

  it("attributes to the origin rule alone when only that filter is on", () => {
    const view = buildSidebarView(fixture, null, NONE, false, true);
    expect(keys(view)).toEqual(["claude#keep", "claude#cmd", "claude#blocked"]);
    expect(view.hidden).toEqual({ external: 2, trivial: 0 });
  });

  it("hides nothing at all with both filters off", () => {
    const view = buildSidebarView(fixture, null, NONE, false, false);
    expect(keys(view)).toHaveLength(fixture.length);
    expect(view.hidden).toEqual({ external: 0, trivial: 0 });
  });
});

describe("buildSidebarView — projectCount, counted before filtering (D3)", () => {
  it("counts projects the filters emptied", () => {
    // Two projects in, one group out. A sidebar deriving its grouped/flat mode
    // from groups.length would flip to flat here and re-indent every row.
    const view = buildSidebarView([
      command("a", "/usage", { projectRoot: "/p/chatter" }),
      app("b", { projectRoot: "/p/work" }),
    ], null, NONE, true, true);
    expect(view.groups).toHaveLength(1);
    expect(view.projectCount).toBe(2);
  });

  it("is the same number whatever the filters are doing", () => {
    const sessions = [
      command("a", "/usage", { projectRoot: "/p/chatter" }),
      session("claude", "b", { projectRoot: "/p/work", size: BIG }),
    ];
    for (const hideTrivial of [true, false]) {
      for (const appOnly of [true, false]) {
        expect(buildSidebarView(sessions, null, NONE, hideTrivial, appOnly).projectCount)
          .toBe(2);
      }
    }
  });

  it("counts the Other bucket as one, however many directories it holds", () => {
    const view = buildSidebarView([
      app("a", { cwd: "/Users/me" }),
      app("b", { cwd: "/tmp/scratch" }),
      app("c", { projectRoot: "/p/work" }),
    ], null, NONE, true, true);
    expect(view.projectCount).toBe(2);
    expect(view.groups.map(g => g.projectPath)).toEqual(["/p/work", OTHER_PROJECT]);
  });

  it("normalises trailing slashes exactly as the grouping does", () => {
    const view = buildSidebarView([
      app("a", { projectRoot: "/a/web" }),
      app("b", { projectRoot: "/a/web/" }),
      app("c", { projectRoot: "/a/web//" }),
    ], null, NONE, true, true);
    expect(view.projectCount).toBe(1);
    expect(view.groups).toHaveLength(1);
  });

  it("keeps a root-level repository apart from Other", () => {
    const view = buildSidebarView([
      app("a", { projectRoot: "" }),
      app("b", { projectRoot: "/" }),
    ], null, NONE, true, true);
    expect(view.projectCount).toBe(2);
  });
});

describe("managedWorktreeName — the one worktree fact the renderer can tell", () => {
  it("names the checkout a session is standing in", () => {
    expect(managedWorktreeName({
      cwd: "/tmp/iface-worktrees/interface-3",
      worktreeHome: "/work/interface",
      worktreePath: "/tmp/iface-worktrees/interface-3",
    })).toBe("interface-3");
  });

  it("drops the nested launch directory rather than naming it", () => {
    // worktreePath is `<checkout>/<path relative to the repo root>`. Reading
    // its basename would label this session "renderer".
    expect(managedWorktreeName({
      cwd: "/tmp/iface-worktrees/interface-3/src/renderer",
      worktreeHome: "/work/interface/src/renderer",
      worktreePath: "/tmp/iface-worktrees/interface-3/src/renderer",
    })).toBe("interface-3");
  });

  it("says nothing once the worktree has been given back", () => {
    // The registry moves cwd home when the checkout is removed. The metadata
    // stays on the record; the claim must not.
    expect(managedWorktreeName({
      cwd: "/work/interface",
      worktreeHome: "/work/interface",
      worktreePath: "/tmp/iface-worktrees/interface-3",
    })).toBeUndefined();
  });

  it("says nothing for a session that never had one", () => {
    expect(managedWorktreeName({ cwd: "/work/interface" })).toBeUndefined();
    expect(managedWorktreeName({
      cwd: "/work/interface", worktreePath: "/tmp/wt/x",
    })).toBeUndefined();
  });

  it("reaches the row it describes", () => {
    const rows = sidebarRows([
      session("claude", "a", {
        cwd: "/tmp/iface-worktrees/interface-3",
        worktreeHome: "/work/interface",
        worktreePath: "/tmp/iface-worktrees/interface-3",
      }),
      session("claude", "b", { cwd: "/work/interface" }),
    ], null);
    expect(rows.find(row => row.key === "claude#a")?.worktree).toBe("interface-3");
    const home = rows.find(row => row.key === "claude#b");
    expect(home).toBeDefined();
    expect(home?.worktree).toBeUndefined();
  });
});

describe("sidebarRowMeta — the row's hover card", () => {
  const NOW = Date.parse("2026-08-11T10:04:00.000Z");
  const at = (over: Partial<SidebarRow> = {}): SidebarRow => ({
    key: "claude#a", title: "Steer verb polish", engine: "claude",
    status: "working", badge: "working", cloud: false, selected: false,
    lastActivity: "2026-08-11T10:00:00.000Z", external: false, ...over,
  });

  it("spells every fact a working row has", () => {
    const meta = sidebarRowMeta(at({ worktree: "steer-polish-2" }),
      { added: 62, removed: 11 }, NOW);
    expect(meta).toEqual({
      status: "Working",
      diff: { added: 62, removed: 11 }, age: "4m", worktree: "steer-polish-2",
    });
  });

  it("omits honestly rather than printing a placeholder", () => {
    const meta = sidebarRowMeta(at({ lastActivity: "not a date" }), undefined, NOW);
    expect(meta.diff).toBeNull();
    expect(meta.age).toBe("");
    expect(meta.worktree).toBe("");
  });

  it("never claims a net-zero diff", () => {
    // The only way to reach +0 −0 is a diff whose two sides matched. Printing
    // it reads as a measurement of the session rather than of that one no-op.
    expect(sidebarRowMeta(at(), { added: 0, removed: 0 }, NOW).diff).toBeNull();
  });

  it("names the state with the word every other surface prints", () => {
    for (const badge of ["waiting", "working", "idle", "failed"] as const) {
      expect(sidebarRowMeta(at({ status: badge, badge }), null, NOW).status)
        .toBe(statusWord(badge));
    }
  });
});

describe("sidebarFilterLabel", () => {
  it("says so when nothing is hidden", () => {
    expect(sidebarFilterLabel({ external: 0, trivial: 0 })).toBe("Showing all sessions");
  });

  it("names the origin rule when only it is taking rows", () => {
    expect(sidebarFilterLabel({ external: 112, trivial: 0 }))
      .toBe("112 from other tools hidden");
  });

  it("names the declutter rule when only it is taking rows", () => {
    expect(sidebarFilterLabel({ external: 0, trivial: 3 })).toBe("3 throwaway hidden");
  });

  it("states the total when both are, and never a per-cause number", () => {
    // Naming one cause while the other is also hiding rows would understate the
    // count in the one line the user has to trust.
    expect(sidebarFilterLabel({ external: 112, trivial: 3 })).toBe("115 hidden");
  });

  it("always quotes the true number of missing rows", () => {
    // The label's number is the sidebar's whole claim, so it is checked against
    // the view that produced it rather than against a literal.
    const sessions: SessionSummary[] = [
      app("keep", { size: BIG }),
      session("claude", "cli", { size: BIG }),
      session("claude", "cli2", { size: BIG }),
      app("cmd", { title: "/usage", titleFromSummary: true }),
    ];
    for (const hideTrivial of [true, false]) {
      for (const appOnly of [true, false]) {
        const view = buildSidebarView(sessions, null, NONE, hideTrivial, appOnly);
        const missing = sessions.length - keys(view).length;
        const label = sidebarFilterLabel(view.hidden);
        if (missing === 0) expect(label).toBe("Showing all sessions");
        else expect(label).toContain(String(missing));
      }
    }
  });
});

describe("sidebarEmptyState", () => {
  it("explains the first-run empty pane instead of claiming there are no sessions", () => {
    // THE case: nothing on the machine is tagged, so the origin filter empties
    // a sidebar of 112 rows. "No agent sessions yet." would be a bug report.
    const { text, hint } = sidebarEmptyState({ external: 112, trivial: 0 });
    expect(text).toBe("No chats started in Interface yet.");
    expect(text).not.toBe("No agent sessions yet.");
    expect(hint).not.toBeNull();
    expect(hint).toContain("112 sessions");
    expect(hint).toContain("Show all");
  });

  it("does NOT claim there are no Interface chats while the declutter rule hides some", () => {
    // THE THIRD FILTER PATH. Attribution makes the two counts disjoint: a row
    // the origin rule could take is counted as `external`, so a row counted as
    // `trivial` beside it is one that rule did NOT apply to — and both rules
    // open with the same idle-and-unrevealed vetoes, so that row's origin is
    // "app". `trivial > 0` alongside `external > 0` is therefore PROOF that
    // chats were started in Interface, and the old headline said the opposite.
    const { text, hint, action } = sidebarEmptyState({ external: 9, trivial: 4 });
    expect(text).not.toContain("No chats started in Interface");
    expect(text).toBe("Both filters are hiding everything here.");
    // Both causes named with their own numbers: the pane offers one control and
    // has to say what turning it off brings back.
    expect(hint).toContain("9 started from your terminal");
    expect(hint).toContain("4 throwaway");
    expect(hint).toContain("Show all");
    expect(action).toBe("show-all");
  });

  it("says session, not sessions, for a lone throwaway beside an external count", () => {
    expect(sidebarEmptyState({ external: 9, trivial: 1 }).hint)
      .toContain("1 throwaway session started here");
  });

  it("says session, not sessions, for one", () => {
    expect(sidebarEmptyState({ external: 1, trivial: 0 }).hint).toContain("1 session started");
  });

  it("keeps the declutter sentence when only that rule emptied the pane", () => {
    const { text, hint } = sidebarEmptyState({ external: 0, trivial: 2 });
    expect(text).toBe("Every session here is a slash command or a scratch run.");
    expect(hint).toContain("Show all");
  });

  it("falls back to the plain sentence when the list is genuinely empty", () => {
    // No filter is hiding anything, so there is nothing to explain — but there
    // is still exactly one thing to do, and the action says which.
    expect(sidebarEmptyState({ external: 0, trivial: 0 }))
      .toEqual({ text: "No agent sessions yet.", hint: null, action: "new" });
  });

  it("names the way out of each empty pane, and it is never the same one", () => {
    // THE DEFECT: the right pane used to pick its sentence from
    // sessions.length alone, so two 100-byte Interface sessions hidden by the
    // DECLUTTER rule produced "No chats started in Interface yet." — the wrong
    // cause, pointing at the wrong control.
    expect(sidebarEmptyState({ external: 0, trivial: 2 }).action).toBe("show-all");
    expect(sidebarEmptyState({ external: 0, trivial: 2 }).text)
      .not.toContain("No chats started in Interface");
    expect(sidebarEmptyState({ external: 112, trivial: 0 }).action).toBe("show-all");
    expect(sidebarEmptyState({ external: 0, trivial: 0 }).action).toBe("new");
  });

  it("answers the search first, because that is the question just asked", () => {
    // A filter's sentence here would explain something nobody asked about.
    const { text, hint, action } =
      sidebarEmptyState({ external: 112, trivial: 3 }, { searching: true });
    expect(text).toBe("No sessions match your search.");
    expect(hint).toBeNull();
    expect(action).toBe("clear-search");
  });

  it("points at the filter when the match is behind it", () => {
    const { text, hint, action } =
      sidebarEmptyState({ external: 112, trivial: 0 }, { searching: true, hiddenMatches: 3 });
    expect(text).toBe("No matches in the sessions shown.");
    expect(hint).toContain("3 hidden sessions match");
    expect(hint).toContain("Show all");
    expect(action).toBe("show-all");
  });

  it("says session, not sessions, for one hidden match", () => {
    expect(sidebarEmptyState({ external: 1, trivial: 0 },
      { searching: true, hiddenMatches: 1 }).hint).toContain("1 hidden session match");
  });
});

describe("buildSidebarView — purity and edges", () => {
  it("does not mutate the input array or its sessions", () => {
    const input: SessionSummary[] = [
      command("a", "/usage"),
      session("claude", "b", { size: BIG }),
    ];
    for (const s of input) Object.freeze(s);
    Object.freeze(input);
    const before = JSON.stringify(input);

    buildSidebarView(input, null, NONE, true, true);
    buildSidebarView(input, null, NONE, false, false);

    expect(JSON.stringify(input)).toBe(before);
  });

  it("is empty for an empty listing, whatever the filters are doing", () => {
    const empty = { groups: [], hidden: { external: 0, trivial: 0 }, hideable: 0,
      projectCount: 0, visible: [], importable: 0, hiddenMatches: 0, searching: false,
      protectedProjects: [], terminalProjects: {} };
    for (const hideTrivial of [true, false]) {
      for (const appOnly of [true, false]) {
        expect(buildSidebarView([], null, NONE, hideTrivial, appOnly)).toEqual(empty);
      }
    }
  });
});

/* ============================================================================
   SEARCH — 112 sessions, findable only by scrolling until now.
   ========================================================================= */

describe("queryTerms", () => {
  it("splits on any whitespace and lowercases", () => {
    expect(queryTerms("  Iface   CLAUDE\tfix ")).toEqual(["iface", "claude", "fix"]);
  });

  it("is empty for a blank query, which is what 'no search' means", () => {
    expect(queryTerms("")).toEqual([]);
    expect(queryTerms("   ")).toEqual([]);
  });
});

describe("sessionMatches", () => {
  const s = session("claude", "a", {
    title: "Refactor the bridge", cwd: "/Users/x/Desktop/Projects/Interface/src",
    projectRoot: "/Users/x/Desktop/Projects/Interface",
  });

  it("matches nothing in particular when there is no query", () => {
    expect(sessionMatches(s, [])).toBe(true);
  });

  it("matches the title, case-insensitively", () => {
    expect(sessionMatches(s, queryTerms("BRIDGE"))).toBe(true);
    expect(sessionMatches(s, queryTerms("compiler"))).toBe(false);
  });

  it("matches the engine id, so 'codex' narrows to one engine", () => {
    expect(sessionMatches(s, queryTerms("claude"))).toBe(true);
    expect(sessionMatches(session("codex", "b"), queryTerms("codex"))).toBe(true);
    expect(sessionMatches(session("codex", "b"), queryTerms("claude"))).toBe(false);
  });

  it("matches the path, which is where the project name lives", () => {
    expect(sessionMatches(s, queryTerms("interface"))).toBe(true);
    expect(sessionMatches(s, queryTerms("desktop/projects"))).toBe(true);
  });

  it("requires EVERY term — terms narrow, they never widen", () => {
    expect(sessionMatches(s, queryTerms("bridge interface"))).toBe(true);
    expect(sessionMatches(s, queryTerms("bridge nonsense"))).toBe(false);
  });

  it("does not fall over on a session with no project root", () => {
    expect(sessionMatches(session("claude", "z", { title: "hi" }), queryTerms("hi"))).toBe(true);
  });
});

describe("buildSidebarView — search", () => {
  const corpus = () => [
    app("keep", { title: "Refactor the bridge", size: BIG }),
    app("other", { title: "Write the docs", size: BIG }),
  ];

  it("drops every row that does not match", () => {
    const view = buildSidebarView(corpus(), null, NONE, true, true, "bridge");
    expect(keys(view)).toEqual(["claude#keep"]);
    expect(view.searching).toBe(true);
  });

  it("changes nothing when the box is empty", () => {
    const view = buildSidebarView(corpus(), null, NONE, true, true, "   ");
    expect(keys(view)).toEqual(["claude#keep", "claude#other"]);
    expect(view.searching).toBe(false);
  });

  it("DOES drop the selected row — unlike the two persistent filters", () => {
    // Deliberate: a search is transient and undone by clearing the field the
    // user is typing in, and the conversation stays open in the right pane.
    const view = buildSidebarView(corpus(), "claude#other", NONE, true, true, "bridge");
    expect(keys(view)).toEqual(["claude#keep"]);
  });

  it("counts a match a FILTER is holding back, so the pane can point at it", () => {
    const sessions = [
      app("mine", { title: "unrelated", size: BIG }),
      session("claude", "theirs", { title: "Refactor the bridge", size: BIG }),  // origin: store
    ];
    const view = buildSidebarView(sessions, null, NONE, true, true, "bridge");
    expect(keys(view)).toEqual([]);
    expect(view.hiddenMatches).toBe(1);
    expect(view.hidden.external).toBe(1);
  });

  it("counts no hidden matches when the filters are off", () => {
    const sessions = [session("claude", "theirs", { title: "bridge", size: BIG })];
    const view = buildSidebarView(sessions, null, NONE, false, false, "bridge");
    expect(view.hiddenMatches).toBe(0);
    expect(keys(view)).toEqual(["claude#theirs"]);
  });

  it("never reports hidden matches when nothing was searched for", () => {
    const sessions = [session("claude", "theirs", { size: BIG })];
    expect(buildSidebarView(sessions, null, NONE, true, true).hiddenMatches).toBe(0);
  });

  it("leaves the hidden counts describing the FILTERS, not the search", () => {
    // The filter row states what the toggles are doing; a search must not
    // inflate that number or the "Show all" offer would lie.
    const sessions = [
      session("claude", "x", { title: "aaa", size: BIG }),
      session("claude", "y", { title: "bbb", size: BIG }),
    ];
    const view = buildSidebarView(sessions, null, NONE, true, true, "aaa");
    expect(view.hidden.external).toBe(2);
    expect(view.hiddenMatches).toBe(1);
  });
});

describe("buildSidebarView — visible", () => {
  it("is exactly the sessions behind the groups, in listing order", () => {
    const sessions = [
      app("a", { projectRoot: "/p/one", size: BIG }),
      app("b", { projectRoot: "/p/two", size: BIG }),
      session("claude", "c", { size: BIG }),
    ];
    const view = buildSidebarView(sessions, null, NONE, true, true);
    expect(view.visible.map(s => s.sessionId)).toEqual(["a", "b"]);
    // Same rows the sidebar draws, so no two consumers can disagree.
    expect(view.visible.map(s => sessionKeyOf(s)).sort())
      .toEqual(keys(view).slice().sort());
  });

  it("is the SessionSummary itself, not a copy", () => {
    const one = app("a", { size: BIG });
    expect(buildSidebarView([one], null, NONE, true, true).visible[0]).toBe(one);
  });
});

function sessionKeyOf(s: SessionSummary): string {
  return `${s.engine}#${s.sessionId}`;
}

/* ============================================================================
   PER-SESSION EVENT-LOG DERIVATIONS
   ========================================================================= */

describe("turnProgress", () => {
  it("is idle when the session is idle and nothing is in flight", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "thinking.signal", active: true },
      { kind: "thinking.signal", active: false },
      { kind: "status.changed", status: "idle" },
    ], "idle", true)).toEqual({ active: false, label: "" });
  });

  it("stays active while tools run even after thinking drops", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "thinking.signal", active: true },
      { kind: "thinking.signal", active: false },
      { kind: "message.delta", text: "I'll look around." },
      { kind: "tool.call", callId: "1", tool: "Read", summary: "Read `/repo/App.tsx`",
        input: {} },
    ], "working", true)).toEqual({
      active: true, label: "Read `/repo/App.tsx`",
    });
  });

  it("says Thinking while the model is reasoning with no open tool", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "thinking.signal", active: true },
    ], "working", true)).toEqual({ active: true, label: "Thinking" });
  });

  it("does not keep Working for a tool that never settled after the turn went idle", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "tool.call", callId: "1", tool: "bash", summary: "npm run stage:update",
        input: {} },
      { kind: "status.changed", status: "idle" },
    ], "working", true)).toEqual({ active: false, label: "" });
  });

  it("clears a closed session so a later liveness flicker cannot resurrect Working", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "thinking.signal", active: true },
      { kind: "tool.call", callId: "1", tool: "bash", summary: "npm test", input: {} },
      { kind: "session.closed" },
    ], "working", true)).toEqual({ active: false, label: "" });
  });

  it("says Waiting for approval when the session is blocked", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "waiting" },
      { kind: "approval.requested", approvalId: "a", tool: "Bash",
        summary: "rm -rf build", input: {} },
    ], "waiting", true)).toEqual({ active: true, label: "Waiting for approval" });
  });

  it("clears Working after a non-retryable error so a stopped Claude turn does not stay active", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "thinking.signal", active: true },
      { kind: "tool.call", callId: "1", tool: "Bash", summary: "ls", input: {} },
      { kind: "error", message: "error_during_execution", retryable: false },
    ], "failed", true)).toEqual({ active: false, label: "" });
  });

  it("keeps Working through a retryable error", () => {
    expect(turnProgress([
      { kind: "status.changed", status: "working" },
      { kind: "error", message: "API retry 1/3", retryable: true },
    ], "working", true)).toEqual({ active: true, label: "Working" });
  });
});

describe("formatTurnElapsed", () => {
  it("prints whole seconds, then minutes and hours", () => {
    expect(formatTurnElapsed(0)).toBe("0s");
    expect(formatTurnElapsed(999)).toBe("0s");
    expect(formatTurnElapsed(1_000)).toBe("1s");
    expect(formatTurnElapsed(59_000)).toBe("59s");
    expect(formatTurnElapsed(60_000)).toBe("1m");
    expect(formatTurnElapsed(72_000)).toBe("1m 12s");
    expect(formatTurnElapsed(3_600_000)).toBe("1h");
    expect(formatTurnElapsed(3_720_000)).toBe("1h 2m");
  });
});

describe("rememberTurnStart", () => {
  it("keeps the original start when the same turn is still running", () => {
    const first = rememberTurnStart({}, "acp:grok#s1", true, 1_000);
    const again = rememberTurnStart(first, "acp:grok#s1", true, 8_000);
    expect(again).toBe(first);
    expect(again).toEqual({ "acp:grok#s1": 1_000 });
  });

  it("forgets the clock only when the turn itself ends", () => {
    const running = rememberTurnStart({}, "claude#s1", true, 1_000);
    expect(rememberTurnStart(running, "claude#s1", false, 4_000)).toEqual({});
  });
});

describe("stampTurnClock", () => {
  it("ignores a missing row or a flicker shorter than half a second", () => {
    expect(stampTurnClock(0, 400, "l:1")).toBeNull();
    expect(stampTurnClock(0, 800, null)).toBeNull();
    expect(stampTurnClock(1_000, 2_200, "l:last")).toEqual({ key: "l:last", ms: 1_200 });
  });
});

describe("sessionActivity", () => {
  it("prefers the NEWEST tool summary", () => {
    expect(sessionActivity([
      { kind: "tool.call", callId: "1", tool: "Bash", summary: "npm test", input: {} },
      { kind: "tool.call", callId: "2", tool: "Read", summary: "read App.tsx", input: {} },
    ])).toBe("read App.tsx");
  });

  it("prefers prose that came AFTER the tool — that is what it is doing now", () => {
    expect(sessionActivity([
      { kind: "tool.call", callId: "1", tool: "Bash", summary: "npm test", input: {} },
      { kind: "message.delta", text: "The suite is green." },
    ])).toBe("The suite is green.");
  });

  it("joins one run of deltas and collapses the whitespace", () => {
    expect(sessionActivity([
      { kind: "message.delta", text: "one\n" },
      { kind: "message.delta", text: "  two" },
    ])).toBe("one two");
  });

  it("reads the LAST run of prose, not the first", () => {
    expect(sessionActivity([
      { kind: "message.delta", text: "opening words" },
      { kind: "tool.result", callId: "1", ok: true, summary: "" },
      { kind: "message.delta", text: "closing words" },
    ])).toBe("closing words");
  });

  it("clips to one bounded line", () => {
    const out = sessionActivity([{ kind: "message.delta", text: "x".repeat(500) }]);
    expect(out).toHaveLength(80);
    expect(out.endsWith("…")).toBe(true);
  });

  it("stops scanning once it has more than it can print", () => {
    // 120 deltas of 20 chars: the guard fires long before the whole log is
    // concatenated, and the answer is still the tail of the conversation.
    const log: EngineEvent[] = [];
    for (let i = 0; i < 120; i++) log.push({ kind: "message.delta", text: "y".repeat(20) });
    expect(sessionActivity(log)).toHaveLength(80);
  });

  it("scans only the head of one coalesced delta", () => {
    // The store folds a streamed answer into ONE delta, so the guard above
    // never fires mid-answer; the clipper must still see a bounded prefix.
    expect(sessionActivity([{
      kind: "message.delta", text: `${"x".repeat(10)}${"\n".repeat(5_000)}y`,
    }])).toBe("xxxxxxxxxx");
  });

  it("skips a tool that summarised nothing", () => {
    expect(sessionActivity([
      { kind: "tool.call", callId: "1", tool: "Bash", summary: "npm test", input: {} },
      { kind: "tool.result", callId: "1", ok: true, summary: "" },
    ])).toBe("npm test");
  });

  it("is empty for a log with nothing to say", () => {
    expect(sessionActivity([])).toBe("");
    expect(sessionActivity([{ kind: "thinking.signal", active: true }])).toBe("");
  });
});

describe("pendingApproval", () => {
  it("returns the newest request", () => {
    expect(pendingApproval([
      { kind: "approval.requested", approvalId: "a1", tool: "Bash", summary: "rm", input: {} },
      { kind: "approval.requested", approvalId: "a2", tool: "Write", summary: "x.ts", input: {} },
    ])).toEqual({ id: "a2", tool: "Write", summary: "x.ts" });
  });

  it("is undefined when none was asked", () => {
    expect(pendingApproval([{ kind: "message.delta", text: "hi" }])).toBeUndefined();
  });

  it("flags AskUserQuestion and uses the question as the summary", () => {
    expect(pendingApproval([
      { kind: "approval.requested", approvalId: "q1", tool: "AskUserQuestion",
        summary: "AskUserQuestion", input: {
          questions: [{
            question: "Which library?", header: "Library", multiSelect: false,
            options: [
              { label: "date-fns", description: "small" },
              { label: "luxon", description: "zones" },
            ],
          }],
        } },
    ])).toEqual({
      id: "q1", tool: "AskUserQuestion", summary: "Which library?", asksQuestion: true,
    });
  });
});

describe("the one blocked-on-you derivation", () => {
  const listing = [
    session("claude", "a", { status: "waiting" }),
    session("codex", "b", { status: "working" }),
    session("claude", "c", { status: "waiting" }),
    session("codex", "d", { status: "failed" }),
    session("claude", "e"),
  ];
  const ALL_STATUSES: SessionStatus[] = ["working", "waiting", "idle", "failed"];

  it("selects waiting sessions and nothing else", () => {
    expect(attentionSessions(listing).map(s => s.sessionId)).toEqual(["a", "c"]);
    expect(attentionSessions([])).toEqual([]);
    // Working and failed are news, but they are not blocking the user; a pill
    // that counted them would send the user somewhere for nothing.
    expect(attentionSessions(listing.filter(s => s.status !== "waiting"))).toEqual([]);
  });

  it("counts and lists from the same selection, so they cannot drift", () => {
    // The pill prints `.length`; navigation is handed the array. A second
    // function that counted separately is exactly the defect this shape bans.
    expect(attentionSessions(listing).length).toBe(2);
    expect(attentionSessions(listing).every(needsAttention)).toBe(true);
  });

  it("agrees with the dock badge on the very same listing", () => {
    // H1's hard constraint: the pill in the window and the badge on the dock
    // icon are two renderings of one fact, and badge.ts is main-process code
    // that cannot import this module. This is the assertion that fails if
    // either predicate is ever changed alone.
    expect(attentionSessions(listing).length).toBe(dockBadgeCount(listing));
    expect(attentionSessions([]).length).toBe(dockBadgeCount([]));
    for (const status of ALL_STATUSES) {
      const one = [session("claude", "only", { status })];
      expect(attentionSessions(one).length).toBe(dockBadgeCount(one));
    }
  });

  it("never counts a session the sidebar's pinned section would not pin", () => {
    // The second surface with a live "Needs attention" count. Its set is
    // deliberately WIDER (it lifts failed rows too), so the two are not equal —
    // but the pill claiming an agent the sidebar does not pin would send the
    // user to a pane that does not hold it. Containment is the invariant, and
    // it fails the moment either predicate moves independently.
    for (const status of ALL_STATUSES) {
      const row: SidebarRow = {
        key: `claude#${status}`, title: status, engine: "claude", status, badge: status,
        cloud: false, selected: false, lastActivity: "2026-08-11T10:00:00.000Z",
        external: false,
      };
      if (!needsAttention({ status })) continue;
      expect(isAttentionRow(row)).toBe(true);
    }
  });

  it("announces the count in a sentence, including its clearing", () => {
    expect(attentionAnnouncement(0)).toBe("No agents need you");
    expect(attentionAnnouncement(1)).toBe("1 agent needs you");
    expect(attentionAnnouncement(4)).toBe("4 agents need you");
  });
});

/* ============================================================================
   ENGINE CAPABILITIES
   ========================================================================= */

describe("engineOpsMap / engineCan", () => {
  const map = engineOpsMap([
    { engine: "claude", ops: ["renameSession", "deleteSession", "adoptSession"] },
    { engine: "codex", ops: ["renameSession", "deleteSession", "setArchived"] },
  ]);

  it("answers per engine, from what the adapters actually reported", () => {
    expect(engineCan(map, "claude", "adoptSession")).toBe(true);
    // Codex genuinely cannot stamp an adoption: thread/metadata/update carries
    // git fields only, so its adapter omits the method by design.
    expect(engineCan(map, "codex", "adoptSession")).toBe(false);
    expect(engineCan(map, "codex", "setArchived")).toBe(true);
    expect(engineCan(map, "claude", "setArchived")).toBe(false);
  });

  it("says NO for an engine nobody reported on", () => {
    // The probe answers before a click is possible; guessing yes puts a button
    // in front of the user that rejects.
    expect(engineCan(map, "acp:whatever", "renameSession")).toBe(false);
    expect(engineCan({}, "claude", "renameSession")).toBe(false);
  });
});

/* ============================================================================
   THE MID-SESSION ACCESS PICKER — every case here is a refusal the user would
   otherwise meet as an error message.
   ========================================================================= */

describe("withoutDefaultPermissionMode", () => {
  it("removes the composer's 'default', which Codex rejects outright", () => {
    // VERIFIED LIVE (codex app-server 0.147.0): approvalPolicy "default"
    // answers -32600 unknown variant, while an omitted policy answers OK. The
    // field must be ABSENT, not empty-string, not "default".
    const out = withoutDefaultPermissionMode(
      { engine: "codex", cwd: "/tmp/p", prompt: "hi", permissionMode: "default" });
    expect("permissionMode" in out).toBe(false);
    expect(out).toEqual({ engine: "codex", cwd: "/tmp/p", prompt: "hi" });
  });

  it("passes a real mode through untouched, for either engine", () => {
    expect(withoutDefaultPermissionMode({ permissionMode: "on-request" }))
      .toEqual({ permissionMode: "on-request" });
    expect(withoutDefaultPermissionMode({ permissionMode: "plan" }))
      .toEqual({ permissionMode: "plan" });
    // The dangerous one especially: dropping it would silently downgrade a
    // launch the user armed in two deliberate steps.
    expect(withoutDefaultPermissionMode({ permissionMode: "bypassPermissions" }))
      .toEqual({ permissionMode: "bypassPermissions" });
  });

  it("leaves a request that never had one alone", () => {
    const req: { cwd: string; permissionMode?: string } = { cwd: "/tmp/p" };
    expect(withoutDefaultPermissionMode(req)).toEqual({ cwd: "/tmp/p" });
  });

  it("does not mutate the caller's object", () => {
    const req = { cwd: "/tmp/p", permissionMode: "default" };
    withoutDefaultPermissionMode(req);
    expect(req.permissionMode).toBe("default");
  });
});

describe("permissionPicker", () => {
  // The two engines' real vocabularies. Claude's is the SDK's PermissionMode
  // union; Codex's is AskForApproval, whose members were read back off the
  // running server (codex app-server 0.147.0).
  const CLAUDE = [
    "default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions",
  ];
  const CODEX = ["untrusted", "on-request", "never"];

  it("drops the dangerous mode from a session that is not running one", () => {
    const p = permissionPicker(CLAUDE, ["bypassPermissions"], "default");
    expect(p.options).toEqual(["default", "acceptEdits", "plan", "auto", "dontAsk"]);
    expect(p.currentUnknown).toBe(false);
  });

  it("KEEPS an armed dangerous mode, first, so it can be seen and disarmed", () => {
    // The whole point: a session launched with bypassPermissions must be able
    // to say so, and picking anything else is how the user turns it off. Drop
    // it from the list and the bar silently misreports an unsupervised agent.
    const p = permissionPicker(CLAUDE, ["bypassPermissions"], "bypassPermissions");
    expect(p.options).toEqual([
      "bypassPermissions", "default", "acceptEdits", "plan", "auto", "dontAsk",
    ]);
    expect(p.currentUnknown).toBe(false);
  });

  it("never lists a mode twice when the current one is already safe", () => {
    expect(permissionPicker(CLAUDE, ["bypassPermissions"], "plan").options)
      .toEqual(["default", "acceptEdits", "plan", "auto", "dontAsk"]);
  });

  it("flags Codex's 'default' as unknown instead of offering it", () => {
    // VERIFIED LIVE against codex app-server 0.147.0: thread/start with
    // approvalPolicy "default" answers -32600 "unknown variant `default`,
    // expected one of `untrusted`, `on-request`, `granular`, `never`". It is
    // what the app shows before anything is chosen, so it must be DISPLAYED —
    // but selecting it would be a guaranteed rejection.
    const p = permissionPicker(CODEX, [], "default");
    expect(p.currentUnknown).toBe(true);
    expect(p.options).toEqual(["untrusted", "on-request", "never"]);
    expect(p.options).not.toContain("default");
  });

  it("switches off the unknown flag once a real mode is running", () => {
    expect(permissionPicker(CODEX, [], "untrusted"))
      .toEqual({ options: ["untrusted", "on-request", "never"], currentUnknown: false });
  });

  it("offers nothing for an engine whose vocabulary is unknown", () => {
    // An engine with no table entry gets no picker at all, rather than one
    // built out of the other engine's words.
    expect(permissionPicker([], ["bypassPermissions"], "default"))
      .toEqual({ options: [], currentUnknown: true });
  });

  it("does not mutate its inputs", () => {
    const modes = [...CLAUDE];
    const dangerous = ["bypassPermissions"];
    permissionPicker(modes, dangerous, "bypassPermissions");
    expect(modes).toEqual(CLAUDE);
    expect(dangerous).toEqual(["bypassPermissions"]);
  });
});

/* ============================================================================
   PLAN USAGE — "not reported" is a real answer, 0% is a lie.
   ========================================================================= */

const NOW = Date.parse("2026-08-12T12:00:00.000Z");

describe("formatReset", () => {
  it("is empty when the engine gave no time, or gave nonsense", () => {
    expect(formatReset(null, NOW)).toBe("");
    expect(formatReset("", NOW)).toBe("");
    expect(formatReset("not a date", NOW)).toBe("");
  });

  it("counts down in minutes, hours and days", () => {
    expect(formatReset("2026-08-12T12:30:00.000Z", NOW)).toBe("resets in 30m");
    expect(formatReset("2026-08-12T14:05:00.000Z", NOW)).toBe("resets in 2h 5m");
    expect(formatReset("2026-08-12T14:00:00.000Z", NOW)).toBe("resets in 2h");
    expect(formatReset("2026-08-15T12:00:00.000Z", NOW)).toBe("resets in 3d");
    expect(formatReset("2026-08-15T16:00:00.000Z", NOW)).toBe("resets in 3d 4h");
  });

  it("says 'now' for a window that has already turned over", () => {
    expect(formatReset("2026-08-12T11:00:00.000Z", NOW)).toBe("resets now");
    expect(formatReset("2026-08-12T12:00:30.000Z", NOW)).toBe("resets now");
  });
});

describe("barWidth", () => {
  it("is zero for an unknown, which is why the TEXT must say 'not reported'", () => {
    expect(barWidth(null)).toBe(0);
  });

  it("clamps a value outside the range rather than inventing one", () => {
    expect(barWidth(-5)).toBe(0);
    expect(barWidth(140)).toBe(100);
    expect(barWidth(76)).toBe(76);
  });
});

describe("buildUsageView", () => {
  it("renders an unknown utilization as 'not reported', never as 0%", () => {
    const [v] = buildUsageView([{
      engine: "claude", available: true,
      windows: [{ label: "extra usage", utilization: null, resetsAt: null }],
    }]);
    expect(v.windows[0].percentText).toBe("not reported");
    expect(v.windows[0].percentText).not.toBe("0%");
    expect(v.windows[0].percent).toBeNull();
  });

  it("carries plan, note, cost and both windows for a healthy answer", () => {
    const [v] = buildUsageView([{
      engine: "claude", available: true, plan: "max", costUsd: 1.5,
      windows: [
        { label: "5-hour", utilization: 10, resetsAt: "2026-08-12T14:00:00.000Z" },
        { label: "7-day", utilization: 76.4, resetsAt: null },
      ],
    }]);
    expect(v.plan).toBe("max");
    expect(v.costText).toBe("$1.50");
    expect(v.windows.map(w => w.percentText)).toEqual(["10%", "76%"]);
    // THE ENGINE'S OWN INSTANT, carried through verbatim — not a countdown
    // baked at read time. The panel formats it against the clock it draws at,
    // which is the whole point: this snapshot outlives the minute it was taken.
    expect(v.windows[0].resetsAt).toBe("2026-08-12T14:00:00.000Z");
    expect(v.windows[1].resetsAt).toBeNull();
    expect(formatReset(v.windows[0].resetsAt, NOW)).toBe("resets in 2h");
    // The same snapshot, four hours later. A stored string would still be
    // counting down to an instant two hours in the past.
    expect(formatReset(v.windows[0].resetsAt, NOW + 4 * 3_600_000)).toBe("resets now");
  });

  it("draws NOTHING for an engine that could not report, and keeps its reason", () => {
    // The experimental Claude usage API may vanish on any SDK bump; this is the
    // shape it degrades to, and it must not render as a full or empty bar.
    const [v] = buildUsageView([{
      engine: "claude", available: false, note: "This build of the CLI does not report usage.",
      windows: [{ label: "5-hour", utilization: 0, resetsAt: null }],
    }]);
    expect(v.windows).toEqual([]);
    expect(v.note).toBe("This build of the CLI does not report usage.");
  });

  it("keeps one row per engine, in the order given", () => {
    const views = buildUsageView([
      { engine: "claude", available: true, windows: [] },
      { engine: "codex", available: false, windows: [] },
    ]);
    expect(views.map(v => v.engine)).toEqual(["claude", "codex"]);
  });

  it("has no cost for an engine that reports none", () => {
    const [v] = buildUsageView([{ engine: "codex", available: true, windows: [] }]);
    expect(v.costText).toBe("");
  });
});

describe("usageFreshness", () => {
  it("is stale with nothing read, and says nothing rather than a made-up age", () => {
    expect(usageFreshness(null, NOW)).toEqual({ text: "", stale: true });
  });

  it("counts up in minutes, hours and days", () => {
    expect(usageFreshness(NOW, NOW).text).toBe("read just now");
    expect(usageFreshness(NOW - 59_000, NOW).text).toBe("read just now");
    expect(usageFreshness(NOW - 12 * 60_000, NOW).text).toBe("read 12m ago");
    expect(usageFreshness(NOW - 5 * 3_600_000, NOW).text).toBe("read 5h ago");
    expect(usageFreshness(NOW - 50 * 3_600_000, NOW).text).toBe("read 2d ago");
  });

  it("goes stale exactly at the threshold, not a moment before", () => {
    expect(usageFreshness(NOW - (USAGE_STALE_MS - 1), NOW).stale).toBe(false);
    expect(usageFreshness(NOW - USAGE_STALE_MS, NOW).stale).toBe(true);
  });

  it("holds a snapshot for five minutes, then asks again", () => {
    // The strip keeps a last-known "~33%" past this line, so a five-minute
    // cutoff no longer blanks the meter. An hour between reads left a morning
    // number standing through a whole session of turns.
    expect(USAGE_STALE_MS).toBe(5 * 60_000);
    expect(usageFreshness(NOW - (5 * 60_000 - 1), NOW).stale).toBe(false);
    expect(usageFreshness(NOW - 5 * 60_000, NOW).stale).toBe(true);
    expect(usageFreshness(NOW - 30 * 60_000, NOW).stale).toBe(true);
  });

  it("clamps a read from the future rather than printing a negative age", () => {
    // A clock that moved under us (sleep, NTP step). "read -3m ago" is worse
    // than useless on the one panel whose job is to say how current it is.
    const { text, stale } = usageFreshness(NOW + 3 * 60_000, NOW);
    expect(text).toBe("read just now");
    expect(stale).toBe(false);
  });
});

describe("usageRailReading", () => {
  const view = (engine: string, windows: { label: string; percent: number | null }[], available = true) => ({
    engine, available, plan: "", note: "", costText: "",
    windows: windows.map(w => ({ ...w, percentText: w.percent === null ? "not reported" : `${w.percent}%`, resetsAt: null })),
  });

  it("shows the fullest window anywhere, named for the rail", () => {
    const reading = usageRailReading([
      view("claude", [{ label: "5-hour", percent: 38 }, { label: "Weekly", percent: 62 }]),
      view("codex", [{ label: "Week", percent: 41 }]),
    ], 1_000, 2_000);
    expect(reading).toMatchObject({ engine: "claude", window: "7d", text: "62%", percent: 62, tone: "ok" });
    expect(usageRailReading([view("codex", [{ label: "5 hour", percent: 91 }])], 1_000, 2_000))
      .toMatchObject({ window: "5h", text: "91%", tone: "high" });
  });

  it("marks a stale figure and never invents one", () => {
    expect(usageRailReading([view("claude", [{ label: "Weekly", percent: 62 }])], 0, USAGE_STALE_MS + 1)?.text)
      .toBe("~62%");
    expect(usageRailReading([view("claude", [{ label: "Weekly", percent: null }])], 0, 1)).toBeNull();
    expect(usageRailReading([view("claude", [{ label: "Weekly", percent: 80 }], false)], 0, 1)).toBeNull();
    expect(usageRailReading([], null, 0)).toBeNull();
  });
});

describe("usageStripItems", () => {
  const THREE = [
    { id: "claude" }, { id: "codex" }, { id: "acp:grok" },
  ];
  const views = buildUsageView([
    { engine: "claude", available: true, windows: [
      { label: "5-hour", utilization: 12, resetsAt: null },
      { label: "7-day", utilization: 81, resetsAt: null },
    ] },
    { engine: "codex", available: true, windows: [
      { label: "weekly", utilization: 100, resetsAt: null },
    ] },
    { engine: "acp:grok", available: false, note: "no plan usage", windows: [] },
  ]);

  it("keeps one chip per advertised provider even before any read", () => {
    const items = usageStripItems([], null, NOW, THREE);
    expect(items.map(item => [item.engine, item.label, item.percentText, item.tone]))
      .toEqual([
        ["claude", "Claude Code", "", "unknown"],
        ["codex", "Codex", "", "unknown"],
        ["acp:grok", "Grok", "", "unknown"],
      ]);
  });

  it("prints each provider's fullest window and never invents a number", () => {
    const items = usageStripItems(views, NOW, NOW, THREE);
    expect(items).toEqual([
      { engine: "claude", label: "Claude Code", percentText: "81%", tone: "high" },
      { engine: "codex", label: "Codex", percentText: "100%", tone: "full" },
      { engine: "acp:grok", label: "Grok", percentText: "", tone: "unknown" },
    ]);
  });

  it("KEEPS a stale percent and marks it approximate rather than blanking it", () => {
    // M1. Blanking turned the always-on meter into three provider words for
    // most of every session — measured at five of eight captured frames. These
    // windows reset over five hours to a week, so the last figure is still the
    // best answer to "can I start three more of these"; the "~" is what stops
    // it asserting more precision than it has.
    const items = usageStripItems(views, NOW - USAGE_STALE_MS, NOW, THREE);
    expect(items.map(item => item.percentText)).toEqual(["~81%", "~100%", ""]);
    expect(items.map(item => item.tone)).toEqual(["high", "full", "unknown"]);
  });

  it("still says nothing for a provider that never reported, however fresh", () => {
    // The one thing worse than a stale number is a tilde in front of a number
    // nobody measured. Grok is available:false in this fixture.
    const stale = usageStripItems(views, NOW - 5 * 3_600_000, NOW, THREE);
    expect(stale[2].percentText).toBe("");
    expect(usageStripItems([], null, NOW, THREE).map(item => item.percentText))
      .toEqual(["", "", ""]);
  });

  it("summarizes the strip for the trigger tooltip", () => {
    expect(usageStripSummary(usageStripItems(views, NOW, NOW, THREE)))
      .toBe("Claude Code 81% · Codex 100% · Grok");
    // Stale, the tooltip carries the marked figures — and the trigger appends
    // the exact age ("read 2h ago") beside them.
    expect(usageStripSummary(usageStripItems(views, NOW - USAGE_STALE_MS, NOW, THREE)))
      .toBe("Claude Code ~81% · Codex ~100% · Grok");
  });
});

describe("usageStripTone", () => {
  it("matches the usage bar's 80 / 100 thresholds", () => {
    expect(usageStripTone(null)).toBe("unknown");
    expect(usageStripTone(79)).toBe("ok");
    expect(usageStripTone(80)).toBe("high");
    expect(usageStripTone(100)).toBe("full");
  });
});

/* ============================================================================
   WHAT THE SELECTED SESSION IS RUNNING
   ========================================================================= */

describe("configInForce", () => {
  const changed = (model: string | undefined, effort: string | null): EngineEvent =>
    ({ kind: "config.changed", model, effort });
  const meta = (model: string): EngineEvent =>
    ({ kind: "session.meta", sessionId: "s", model, cwd: "/tmp/proj" });
  const closed: EngineEvent = { kind: "session.closed" };

  it("is empty for a session that has said nothing", () => {
    expect(configInForce([])).toEqual({ hasConfig: false, model: undefined, effort: undefined });
  });

  it("takes the newest config.changed, both fields together", () => {
    const out = configInForce([changed("opus", "low"), changed("sonnet", "high")]);
    expect(out).toEqual({ hasConfig: true, model: "sonnet", effort: "high" });
  });

  it("lets a config.changed carrying no model win over an older session.meta", () => {
    // `model: undefined` is the user picking the engine's own default, and
    // hasConfig is what stops the caller resurrecting the launch model.
    const out = configInForce([meta("opus"), changed(undefined, null)]);
    expect(out).toEqual({ hasConfig: true, model: undefined, effort: null });
  });

  it("falls back to session.meta when nothing has been changed", () => {
    expect(configInForce([meta("opus")]))
      .toEqual({ hasConfig: false, model: "opus", effort: undefined });
  });

  it("STOPS at the run boundary: a config from before a session.closed is not in force",
    () => {
    // The event log is never cleared, so it outlives the handle that produced
    // it. Durable model/effort restoration comes from SessionSummary, not from
    // replaying an event that belonged to the closed handle.
    const out = configInForce([changed("opus", "high"), closed]);
    expect(out).toEqual({ hasConfig: false, model: undefined, effort: undefined });
  });

  it("reads the resumed run's own config, not the one before the close", () => {
    const out = configInForce([changed("opus", "high"), closed, changed("sonnet", "low")]);
    expect(out).toEqual({ hasConfig: true, model: "sonnet", effort: "low" });
  });

  it("ignores a session.meta from before the close too", () => {
    expect(configInForce([meta("opus"), closed]).model).toBeUndefined();
  });

  it("is unaffected by a close that a later run has already superseded", () => {
    const out = configInForce([closed, meta("opus"), changed("opus", "max")]);
    expect(out).toEqual({ hasConfig: true, model: "opus", effort: "max" });
  });

  it("reads nothing but config, meta and the boundary", () => {
    const noise: EngineEvent[] = [
      { kind: "message.delta", text: "hi" },
      changed("opus", "low"),
      { kind: "status.changed", status: "idle" },
    ];
    expect(configInForce(noise)).toEqual({ hasConfig: true, model: "opus", effort: "low" });
  });
});

describe("fastModeInForce", () => {
  it("uses the latest provider state and preserves its reason", () => {
    expect(fastModeInForce([
      { kind: "fastmode.changed", state: "on" },
      { kind: "fastmode.changed", state: "cooldown", reason: "pending" },
    ])).toEqual({ state: "cooldown", reason: "pending" });
  });

  it("does not carry a prior handle's state across session close", () => {
    expect(fastModeInForce([
      { kind: "fastmode.changed", state: "on" },
      { kind: "session.closed" },
    ])).toBeNull();
    expect(fastModeInForce([
      { kind: "fastmode.changed", state: "on" },
      { kind: "session.closed" },
      { kind: "fastmode.changed", state: "off" },
    ])).toEqual({ state: "off" });
  });
});

/* ============================================================================
   IMPORT
   ========================================================================= */

describe("importProgressLabel", () => {
  it("counts through the batch", () => {
    expect(importProgressLabel({ adopt: true, done: 42, total: 170 }))
      .toBe("Importing 42 of 170…");
  });

  it("names the other direction honestly", () => {
    expect(importProgressLabel({ adopt: false, done: 3, total: 9 }))
      .toBe("Removing 3 of 9…");
  });

  it("does not print '0 of 0' before the total is known", () => {
    expect(importProgressLabel({ adopt: true, done: 0, total: 0 })).toBe("Importing…");
  });
});

describe("importSummary", () => {
  it("counts the locally-marked rows as done, because they are", () => {
    expect(importSummary({ adopt: true, total: 112, ok: 87, local: 25, failed: 0 }))
      .toContain("Imported 112 of 112 sessions.");
  });

  it("says out loud which ones a reinstall would forget", () => {
    const text = importSummary({ adopt: true, total: 112, ok: 87, local: 25, failed: 0 });
    expect(text).toContain("25 marked in Interface only");
    expect(text).toContain("reinstall");
  });

  it("stays quiet about local rows when there are none", () => {
    expect(importSummary({ adopt: true, total: 87, ok: 87, local: 0, failed: 0 }))
      .toBe("Imported 87 of 87 sessions.");
  });

  it("reports failures rather than hiding them in the total", () => {
    const text = importSummary({ adopt: true, total: 10, ok: 7, local: 0, failed: 3 });
    expect(text).toContain("Imported 7 of 10 sessions.");
    expect(text).toContain("3 failed.");
  });

  it("names the direction", () => {
    expect(importSummary({ adopt: false, total: 5, ok: 5, local: 0, failed: 0 }))
      .toBe("Removed 5 of 5 sessions.");
  });
});
