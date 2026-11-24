import { describe, expect, it } from "vitest";
import fixtureV42 from "../fixtures/threads-v42.json";
import fixtureV45 from "../fixtures/threads-v45.json";
import fixture from "../fixtures/threads-v34.json";
import fixtureV69 from "../fixtures/threads-totals-v69.json";
import { sessionKey, type EngineEvent, type SessionStatus, type SessionSummary } from "../shared/engine";
import { sameThreadLiveFacts, threadLiveFacts, type ThreadLiveFacts } from "./session-activity";
import { buildSidebarView } from "./sidebar-view";
import { threadChangeTotals } from "./thread-pane";
import type { ThreadView, ThreadsView } from "../shared/threads";
import {
  adjacentThreadKey, ALL_THREAD_PROJECTS, buildThreadInbox, DEFAULT_THREAD_FOLDS,
  highestThreadMark, NO_THREAD_PROJECT, parseThreadFolds, parseThreadScope, SETTLED_PAGE_SIZE,
  snoozePresets, snoozeWakeLabel, THREAD_MENU_CHORDS, threadActionLeavesSection, threadActivityAt,
  threadAgeLabel, threadChord, threadDiffTotals, threadDropCommands, threadDropUndo, threadElapsedLabel,
  threadFocusAfterRemoval, threadRowAccessibleName, threadRowSlot, threadRowTrail, threadShelfSlot,
  threadTitleTone, threadWakeTime,
  threadInboxOrder, threadMark, threadMarkWord, threadMenuEntries, threadMoveEntries,
  threadKeptText, threadKeptWorktrees, threadProjectRoot, threadRunEnds, threadScopeFor,
  threadScopeOptions, threadScopeRoot, threadSectionAccessibleName, threadSectionRows,
  threadListingRevealed, threadPullRequestFact, threadTracksPullRequest, threadUndo, threadUndoCommands,
  threadUndoMessage, THREAD_META_LINE, threadDropBeforeKey, threadNeedsYouFirst, threadRowLine,
  UNRECORDED_THREAD_GRACE_MS, threadDrawnSection, threadHandoffTask, threadTerminalRefusal,
  type ThreadInboxInput, type ThreadInboxRow, type ThreadMenuCommand, type ThreadMenuSubmenu,
  IN_MESSAGES_LABEL, threadInitials, threadMessageExcerpts,
} from "./thread-inbox";
import type { JournalHit } from "../shared/thread-journal";

const VIEW = fixture.view as ThreadsView;
const ATLAS = "/Users/owner/Developer/Projects/atlas";
const PINNED = "claude#5f0c1d2e-8a4b-4c6d-9e7f-1a2b3c4d5e6f";
const ACTIVE = "codex#019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90";
const SNOOZED = "codex#019a3d70-1b42-7e55-8c11-5f3a9d2e7b04";
const SETTLED = "claude#c8e1f2a3-4b5c-4d6e-8f70-91a2b3c4d5e6";

function session(key: string, over: Partial<SessionSummary> = {}): SessionSummary {
  const [engine, sessionId] = key.split("#") as [SessionSummary["engine"], string];
  return {
    engine, sessionId, title: `Thread ${sessionId.slice(0, 4)}`, cwd: "/tmp/private",
    status: "idle", cloud: false, origin: "app",
    firstSeen: "2026-10-01T00:00:00.000Z", lastActivity: "2026-10-01T00:00:00.000Z",
    archived: false, ...over,
  };
}

// The fixture's four threads as the registry would list them: a worktree
// thread runs inside the worktree, not the project the owner picked.
const SESSIONS: SessionSummary[] = [
  session(PINNED, { title: "Session cache invalidation", projectRoot: ATLAS, cwd: ATLAS, kind: "code" }),
  session(ACTIVE, { title: "Fix the flaky login test", kind: "code",
    cwd: "/Users/owner/Developer/Projects/.interface-worktrees/fix-the-flaky-login-test-3f9a2c",
    projectRoot: "/Users/owner/Developer/Projects/.interface-worktrees/fix-the-flaky-login-test-3f9a2c" }),
  session(SNOOZED, { title: "Regression test for retry", kind: "code" }),
  session(SETTLED, { title: "Draft release notes for 0.4", kind: "chat" }),
];

function inbox(over: Partial<ThreadInboxInput> = {}) {
  return buildThreadInbox({
    sessions: SESSIONS, view: VIEW, scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null, ...over,
  });
}

function keysBySection(model: ReturnType<typeof buildThreadInbox>): Record<string, string[]> {
  return Object.fromEntries(model.sections.map(section => [section.section, section.rows.map(row => row.key)]));
}

function thread(key: string, over: Partial<ThreadView> = {}): ThreadView {
  return { key, section: "active", createdAt: 0, woke: false, unread: false, autoSettle: true, turns: [], ...over };
}

function view(threads: ThreadView[]): ThreadsView {
  return { ...VIEW, threads };
}

function row(over: Partial<ThreadInboxRow> = {}): ThreadInboxRow {
  return {
    key: "codex#a", title: "A", displayTitle: "A", engine: "codex", status: "idle", projectRoot: null, projectName: "",
    projectLabel: "No project", activityAt: 0, sessionRoot: "/tmp/private",
    section: "active", mark: "none", selected: false, known: true, pinned: false, unread: false,
    autoSettle: true, createdAt: 0, lastActivity: "2026-10-01T00:00:00.000Z", external: false,
    cloud: false, ...over,
  };
}

describe("the Threads inbox model", () => {
  it("draws Rust's four sections from the shared v34 view, in inbox order", () => {
    const model = inbox();
    expect(model.sections.map(section => section.label)).toEqual(["Pinned", "Active", "Snoozed", "Settled"]);
    expect(keysBySection(model)).toEqual({
      pinned: [PINNED], active: [ACTIVE], snoozed: [SNOOZED], settled: [SETTLED],
    });
    expect(model.total).toBe(4);
    const active = model.sections[1].rows[0];
    expect(active).toMatchObject({ known: true, unread: true, autoSettle: false, projectName: "atlas",
      projectRoot: ATLAS, mark: "failed" });
    expect(model.sections[3].rows[0]).toMatchObject({ projectRoot: null, projectName: "", mark: "none" });
  });

  it("hides empty sections", () => {
    const model = inbox({ sessions: SESSIONS.filter(item => item.sessionId.startsWith("019a3c5e")) });
    expect(model.sections.map(section => section.section)).toEqual(["active"]);
  });

  it("orders Pinned and Active by Rust's keys, unkeyed rows after them newest first", () => {
    const keys = ["codex#k1", "codex#k2", "codex#u-old", "codex#u-new", "codex#unknown"];
    const model = buildThreadInbox({
      sessions: keys.map(key => session(key, key === "codex#unknown"
        ? { firstSeen: "2026-10-05T00:00:00.000Z" } : {})),
      view: view([
        // Base-62 order is code-unit order: "Zz" sorts before "a0".
        thread("codex#k1", { activeOrder: "a0", createdAt: 1 }),
        thread("codex#k2", { activeOrder: "Zz", createdAt: 2 }),
        thread("codex#u-old", { createdAt: 1_000 }),
        thread("codex#u-new", { createdAt: 3_000_000_000_000 }),
      ]),
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
      now: Date.parse("2026-10-05T00:02:00.000Z"),
    });
    // A session Rust has no record for yet is Active, created at firstSeen.
    expect(keysBySection(model).active)
      .toEqual(["codex#k2", "codex#k1", "codex#u-new", "codex#unknown", "codex#u-old"]);
    const pinned = buildThreadInbox({
      sessions: ["codex#p1", "codex#p2", "codex#p3"].map(key => session(key)),
      view: view([
        thread("codex#p1", { section: "pinned", pinnedAt: 10 }),
        thread("codex#p2", { section: "pinned", pinnedAt: 20 }),
        thread("codex#p3", { section: "pinned", pinnedAt: 5, pinOrder: "V" }),
      ]),
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
    });
    expect(keysBySection(pinned).pinned).toEqual(["codex#p3", "codex#p2", "codex#p1"]);
  });

  it("orders Snoozed by wake time and Settled newest settled first", () => {
    const model = buildThreadInbox({
      sessions: ["codex#s1", "codex#s2", "codex#t1", "codex#t2"].map(key => session(key)),
      view: view([
        thread("codex#s1", { section: "snoozed", snoozedUntil: 900 }),
        thread("codex#s2", { section: "snoozed", snoozedUntil: 100 }),
        thread("codex#t1", { section: "settled", settledAt: 5 }),
        thread("codex#t2", { section: "settled", settledAt: 50 }),
      ]),
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
    });
    expect(keysBySection(model)).toEqual({ snoozed: ["codex#s2", "codex#s1"], settled: ["codex#t2", "codex#t1"] });
  });

  it("treats every session as Active, never unread, while Rust has said nothing", () => {
    const model = inbox({ view: null });
    expect(model.sections.map(section => section.section)).toEqual(["active"]);
    expect(model.sections[0].rows.every(item => !item.known && !item.unread && item.mark === "none")).toBe(true);
    expect(model.settings).toBeNull();
    // Newest firstSeen first.
    const ordered = inbox({ view: null, sessions: [
      session("codex#old", { firstSeen: "2026-01-01T00:00:00.000Z" }),
      session("codex#new", { firstSeen: "2026-09-01T00:00:00.000Z" }),
    ] });
    expect(keysBySection(ordered).active).toEqual(["codex#new", "codex#old"]);
  });

  it("hides a session Rust has no record for once Rust has answered, after five minutes", () => {
    const now = Date.parse("2026-10-05T12:00:00.000Z");
    const ago = (ms: number): string => new Date(now - ms).toISOString();
    const sessions = [
      session(ACTIVE),
      session("codex#new", { firstSeen: ago(UNRECORDED_THREAD_GRACE_MS - 1_000) }),
      session("codex#worker", { firstSeen: ago(UNRECORDED_THREAD_GRACE_MS + 1_000), projectRoot: "/w/hidden" }),
      session("codex#unreadable", { firstSeen: "never" }),
    ];
    const loaded = buildThreadInbox({ sessions, view: view([thread(ACTIVE, { activeOrder: "a0" })]),
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null, now });
    // Still on its way: Active, as before Rust answered. A hidden worker is gone,
    // and does not put its project in the scope picker either.
    expect(keysBySection(loaded)).toEqual({ active: [ACTIVE, "codex#new"] });
    expect(loaded.sections[0].rows[1]).toMatchObject({ known: false, unread: false, mark: "none" });
    expect(loaded.projects.map(project => project.root)).not.toContain("/w/hidden");
    // Before Rust answers, nothing is hidden.
    const unloaded = buildThreadInbox({ sessions, view: null,
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null, now });
    expect(unloaded.total).toBe(4);
  });

  it("keeps a thread Rust records out of the sidebar's small-transcript declutter", () => {
    // Hands-free run, 6 Oct 2026: after a restart a pinned thread, a snoozed
    // one and a plain chat vanished from the inbox — their transcripts were
    // under the declutter size and the run's reveals were gone.
    const small = { origin: "app" as const, size: 4_000 };
    const pinned = session("claude#pinned", small);
    const unrecorded = session("claude#unrecorded", small);
    const cli = session("claude#cli", { origin: "store", size: 4_000 });
    const sessions = [pinned, unrecorded, cli];
    const recorded = view([thread("claude#pinned", { section: "pinned", pinnedAt: 1 }),
      thread("claude#cli", { section: "settled", settledReason: "external" })]);
    const none = new Set<string>();
    const revealed = threadListingRevealed(none, sessions, recorded);
    expect([...revealed]).toEqual(["claude#pinned"]);
    const listed = buildSidebarView(sessions, null, revealed, true, true).visible
      .map(item => sessionKey(item.engine, item.sessionId));
    // The recorded thread stays; an unrecorded small one is still decluttered,
    // and provider history is still left out by "started here only".
    expect(listed).toEqual(["claude#pinned"]);
    // Nothing to add keeps the caller's set, so its identity holds.
    expect(threadListingRevealed(none, sessions, null)).toBe(none);
    expect(threadListingRevealed(revealed, sessions, recorded)).toBe(revealed);
  });

  it("lists plain chats and leaves Agent tasks out", () => {
    const model = inbox({ view: null, sessions: [
      session("claude#plain", { kind: "chat" }),
      session("claude#voice", { kind: "voice" as SessionSummary["kind"] }),
      session("claude#task", { kind: "agent" }),
    ] });
    expect(keysBySection(model).active.sort()).toEqual(["claude#plain", "claude#voice"]);
  });

  it("files a thread under its own workspace's project, else the session's repository", () => {
    expect(threadProjectRoot({ projectRoot: "/elsewhere/" },
      { workspace: { mode: "worktree", projectRoot: `${ATLAS}/`, owned: true } })).toBe(ATLAS);
    expect(threadProjectRoot({ projectRoot: ATLAS }, { workspace: { mode: "none", projectRoot: "", owned: false } }))
      .toBeNull();
    expect(threadProjectRoot({ projectRoot: `${ATLAS}//` })).toBe(ATLAS);
    expect(threadProjectRoot({ projectRoot: "" })).toBeNull();
    expect(threadProjectRoot({})).toBeNull();
  });

  it("narrows to one project, or to No project, and keeps the scope's project listed", () => {
    expect(keysBySection(inbox({ scope: threadScopeFor(ATLAS) })))
      .toEqual({ pinned: [PINNED], active: [ACTIVE], snoozed: [SNOOZED] });
    const none = inbox({ scope: NO_THREAD_PROJECT });
    // The snoozed thread has a worktree workspace in the fixture, so only the
    // settled plain chat is No project.
    expect(keysBySection(none)).toEqual({ settled: [SETTLED] });
    expect(none.unscoped).toBe(4);
    const gone = inbox({ scope: threadScopeFor("/gone/web") });
    expect(gone.total).toBe(0);
    expect(gone.projects.map(project => project.root)).toEqual([ATLAS, "/gone/web"]);
    expect(inbox().projects).toEqual([{ root: ATLAS, name: "atlas" }]);
    expect(inbox().hasNoProject).toBe(true);
  });

  it("searches titles, engines and the thread's project name, before the scope", () => {
    // The worktree thread's directory does not carry "atlas"; its workspace does.
    expect(keysBySection(inbox({ query: "atlas flaky" }))).toEqual({ active: [ACTIVE] });
    expect(keysBySection(inbox({ query: "release" }))).toEqual({ settled: [SETTLED] });
    expect(inbox({ query: "CLAUDE" }).total).toBe(2);
    const scoped = inbox({ query: "release", scope: threadScopeFor(ATLAS) });
    expect(scoped.total).toBe(0);
    expect(scoped.unscoped).toBe(1);
    expect(inbox({ query: "release" }).searching).toBe(true);
    expect(inbox().searching).toBe(false);
  });

  it("lists threads whose messages match under In messages, with the excerpt in the project's place", () => {
    const hit = (key: string, excerpt: string, seq = 1): JournalHit =>
      ({ key, seq, role: "message", excerpt, at: 0 });
    const messageHits = [
      hit(SNOOZED, "…the retry loop never backs off…", 9),
      hit(SNOOZED, "an older match", 3),
      // Its row already matches by title: not listed twice.
      hit(ACTIVE, "flaky in CI"),
      // Not a thread the listing has.
      hit("codex#gone", "lost"),
      hit(PINNED, "Invalidate on retry", 2),
    ];
    const model = inbox({ query: "retry flaky", messageHits });
    expect(keysBySection(model)).toEqual({});
    const found = inbox({ query: "flaky", messageHits });
    expect(keysBySection(found)).toEqual({ active: [ACTIVE] });
    expect(found.inMessages.map(row => row.key)).toEqual([SNOOZED, PINNED]);
    const [snoozed, pinned] = found.inMessages;
    // Ordinary Active rows: the excerpt where the project was, nothing else on line two.
    expect(snoozed).toMatchObject({ section: "active", projectLabel: "…the retry loop never backs off…",
      title: "Regression test for retry" });
    expect(pinned.projectLabel).toBe("Invalidate on retry");
    expect(pinned.branch).toBeUndefined();
    expect(pinned.diff).toBeUndefined();
    expect(pinned.pullRequest).toBeUndefined();
    // They do not count as rows the search kept, and the scope still narrows them.
    expect(found.total).toBe(1);
    expect(inbox({ query: "flaky", messageHits, scope: threadScopeFor(ATLAS) }).inMessages.map(row => row.key))
      .toEqual([SNOOZED, PINNED]);
    expect(inbox({ query: "flaky", messageHits, scope: NO_THREAD_PROJECT }).inMessages).toEqual([]);
    // Under three characters, or with no search, the journal is not drawn.
    expect(inbox({ query: "fl", messageHits }).inMessages).toEqual([]);
    expect(inbox({ messageHits }).inMessages).toEqual([]);
    expect(inbox({ query: "flaky" }).inMessages).toEqual([]);
    expect(IN_MESSAGES_LABEL).toBe("In messages");
    expect([...threadMessageExcerpts("  retry ", messageHits)]).toEqual([
      [SNOOZED, "…the retry loop never backs off…"], [ACTIVE, "flaky in CI"], ["codex#gone", "lost"],
      [PINNED, "Invalidate on retry"],
    ]);
  });

  it("starts an In messages excerpt just before the match, so the matching text shows on line two", () => {
    // Rust keeps about 40 characters before the match; line two shows about 14.
    const excerpt = "…2. You wrote: > second prompt mentions albatross No files changed: this chat has no tools.";
    const messageHits: JournalHit[] = [{ key: SNOOZED, seq: 5, role: "message", excerpt, at: 0 }];
    const [row] = inbox({ query: "Albatross", messageHits }).inMessages;
    expect(row.projectLabel).toBe("…albatross No files changed: this chat has no tools.");
    // A match near the start keeps the excerpt whole, and so does one the excerpt does not show.
    const near: JournalHit[] = [{ key: SNOOZED, seq: 5, role: "message", excerpt: "Old albatross notes", at: 0 }];
    expect(inbox({ query: "albatross", messageHits: near }).inMessages[0].projectLabel).toBe("Old albatross notes");
    expect(inbox({ query: "pelican", messageHits: near }).inMessages[0].projectLabel).toBe("Old albatross notes");
  });

  it("marks the selected thread", () => {
    expect(inbox({ selectedKey: SNOOZED }).sections[2].rows[0].selected).toBe(true);
    expect(inbox({ selectedKey: SNOOZED }).sections[0].rows[0].selected).toBe(false);
  });
});

describe("the status ladder", () => {
  const ended = (outcome: "completed" | "failed") => ({ startedAt: 1, endedAt: 2, outcome });
  it.each<[string, SessionStatus, boolean, Partial<ThreadView> | undefined, string]>([
    ["needs you beats working", "waiting", false, { unread: true, woke: true }, "needs-you"],
    ["a pending approval passed in counts as needs you", "working", true, undefined, "needs-you"],
    ["working beats failed and unread", "working", false, { unread: true, lastRun: ended("failed") }, "working"],
    ["a live failure", "failed", false, undefined, "failed"],
    ["an unread failed run", "idle", false, { unread: true, lastRun: ended("failed") }, "failed"],
    ["an unread done run is the blue dot", "idle", false, { unread: true, lastRun: ended("completed") }, "unread"],
    ["unread beats woke", "idle", false, { unread: true, woke: true }, "unread"],
    ["a snoozed thread back", "idle", false, { woke: true }, "woke"],
    ["a visited failed run is quiet", "idle", false, { unread: false, lastRun: ended("failed") }, "none"],
    ["nothing known", "idle", false, undefined, "none"],
  ])("%s", (_name, status, needsYou, record, expected) => {
    expect(threadMark(status, needsYou, record === undefined ? undefined : thread("codex#x", record)))
      .toBe(expected);
  });

  it("names each mark, and a folded section shows its highest", () => {
    expect(threadMarkWord("needs-you")).toBe("Needs attention");
    expect(threadMarkWord("unread")).toBe("Done");
    expect(threadMarkWord("woke")).toBe("Back from snooze");
    expect(threadMarkWord("none")).toBe("");
    expect(highestThreadMark(["unread", "woke", "failed", "none"])).toBe("failed");
    expect(highestThreadMark([])).toBe("none");
    const model = inbox({ sessions: SESSIONS.map(item => item.sessionId.startsWith("c8e1")
      ? { ...item, status: "waiting" as const } : item) });
    const settled = model.sections[3];
    expect(settled.mark).toBe("needs-you");
    expect(threadSectionAccessibleName(settled, true)).toBe("Settled, 1 thread, needs attention");
    expect(threadSectionAccessibleName(settled, false)).toBe("Settled, 1 thread");
  });
});

describe("an Away loop's thread in the inbox (review B3)", () => {
  it("marks the row of a thread an Away loop runs as, and names it so", () => {
    const away = fixtureV45.view as ThreadsView;
    const key = away.threads[0]!.key;
    const model = buildThreadInbox({ sessions: [session(key, { title: "Away · Fix the flaky login test",
      kind: "code" })], view: away, scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null });
    const awayRow = model.sections[0]!.rows[0]!;
    expect(awayRow.away).toBe(true);
    expect(threadRowAccessibleName(awayRow, threadRowSlot(awayRow, 0)))
      .toMatch(/^Away · Fix the flaky login test, Away loop, /u);
    // Every other thread is drawn as it was.
    expect(inbox().sections.flatMap(section => section.rows).every(item => item.away === undefined)).toBe(true);
    expect(threadRowAccessibleName(row(), threadRowSlot(row(), 0))).not.toContain("Away loop");
  });
});

describe("what a row draws", () => {
  it("carries each row's second line, diff, pull request, age source and run start", () => {
    const model = inbox();
    const [pinned, active, snoozed, settled] = model.sections.map(section => section.rows[0]);
    // The worktree thread: its branch, its PR, and every reported turn summed.
    expect(active).toMatchObject({ projectLabel: "atlas", branch: "iface/fix-the-flaky-login-test-3f9a2c",
      diff: { added: 59, removed: 12 }, pullRequest: { number: 482, state: "open" },
      // Its run ended after the registry's lastActivity: the newer wins.
      activityAt: 1791297730000 });
    expect(active.runStartedAt).toBeUndefined();
    // A run still going: its start, for the elapsed clock. A local thread
    // without a branch draws none; a measured 0/0 turn is a known zero.
    expect(pinned).toMatchObject({ runStartedAt: 1791298500000, diff: { added: 0, removed: 0 } });
    expect(pinned.branch).toBeUndefined();
    // A plain chat is "No project", with no branch; no turn reported counts.
    expect(settled).toMatchObject({ projectRoot: null, projectLabel: "No project" });
    expect(settled.branch).toBeUndefined();
    expect(settled.diff).toBeUndefined();
    expect(snoozed.snoozedUntil).toBe(1791363600000);
    // A session Rust has no record for: no diff, no PR, aged by its own activity.
    const unknown = inbox({ view: null }).sections[0].rows.find(item => item.key === ACTIVE)!;
    expect(unknown.diff).toBeUndefined();
    expect(unknown.pullRequest).toBeUndefined();
    expect(unknown.activityAt).toBe(Date.parse("2026-10-01T00:00:00.000Z"));
  });

  it("sums only the turns that reported both counts, and knows when none did", () => {
    expect(threadDiffTotals([{ added: 3, removed: 1 }, {}, { added: 2, removed: 0 }]))
      .toEqual({ added: 5, removed: 1 });
    // Half a count is not a count.
    expect(threadDiffTotals([{ added: 3 }, { removed: 2 }])).toBeUndefined();
    expect(threadDiffTotals([])).toBeUndefined();
    expect(threadDiffTotals([{ added: 0, removed: 0 }])).toEqual({ added: 0, removed: 0 });
  });

  it("gives the pane head's Changes button the same totals as the inbox row", () => {
    const cases: { added?: number; removed?: number }[][] = [
      [], [{}], [{ added: 3, removed: 1 }, {}, { added: 2, removed: 0 }],
      [{ added: 3 }, { removed: 2 }], [{ added: 4, removed: 1 }, { added: 3 }],
    ];
    for (const turns of cases) {
      const head = threadChangeTotals({ turns: turns.map((counts, ordinal) => ({ ordinal, sentAt: ordinal,
        promptDigest: "", hasStart: true, hasEnd: true, state: "done" as const, ...counts })) });
      expect(head).toEqual(threadDiffTotals(turns) ?? null);
    }
  });

  it("totals a switched thread's every segment in the head and the row, and an unswitched one as before", () => {
    const view = fixtureV69.view as ThreadsView;
    const [switched, plain] = view.threads as [ThreadView, ThreadView];
    // Claude +7 −2, Grok nothing measured, the Codex head's own +3 −1.
    expect(threadChangeTotals(switched)).toEqual({ added: 10, removed: 3 });
    expect(threadChangeTotals(plain)).toEqual(threadDiffTotals(plain.turns) ?? null);
    expect(threadChangeTotals(plain)).toEqual({ added: 4, removed: 0 });
    const model = inbox({ view, sessions: [session(switched.key), session(plain.key)] });
    const rows = model.sections.flatMap(section => section.rows);
    expect(rows.find(row => row.key === switched.key)?.diff).toEqual({ added: 10, removed: 3 });
    expect(rows.find(row => row.key === plain.key)?.diff).toEqual({ added: 4, removed: 0 });
  });

  it("ages a row by the newer of its session's activity and its last run's end", () => {
    const at = (iso: string): number => Date.parse(iso);
    const activity = { lastActivity: "2026-10-05T10:00:00.000Z" };
    expect(threadActivityAt(activity, { lastRun: { startedAt: 1, endedAt: at("2026-10-05T11:00:00.000Z") } }, 5))
      .toBe(at("2026-10-05T11:00:00.000Z"));
    expect(threadActivityAt(activity, { lastRun: { startedAt: 1, endedAt: at("2026-10-05T09:00:00.000Z") } }, 5))
      .toBe(at("2026-10-05T10:00:00.000Z"));
    // A run still going has no end: the session's own clock speaks.
    expect(threadActivityAt(activity, { lastRun: { startedAt: at("2026-10-05T12:00:00.000Z") } }, 5))
      .toBe(at("2026-10-05T10:00:00.000Z"));
    // Nothing known: when it was created; not even that, 0 (drawn as nothing).
    expect(threadActivityAt({ lastActivity: "never" }, undefined, 42)).toBe(42);
    expect(threadActivityAt({ lastActivity: "never" }, undefined, 0)).toBe(0);
  });

  it("puts one thing in line one's slot, from the ladder", () => {
    const now = 1_000_000_000;
    const base = { activityAt: now - 5 * 60_000 };
    expect(threadRowSlot({ ...base, mark: "needs-you" }, now)).toEqual({ kind: "status", mark: "needs-you", word: "Needs you" });
    expect(threadRowSlot({ ...base, mark: "working", runStartedAt: now - 161_000 }, now))
      .toEqual({ kind: "status", mark: "working", word: "Working", elapsed: "2:41" });
    // A working session without an open run Rust saw: the word, no clock.
    expect(threadRowSlot({ ...base, mark: "working" }, now)).toEqual({ kind: "status", mark: "working", word: "Working" });
    expect(threadRowSlot({ ...base, mark: "failed" }, now)).toEqual({ kind: "status", mark: "failed", word: "Failed" });
    expect(threadRowSlot({ ...base, mark: "woke" }, now)).toEqual({ kind: "status", mark: "woke", word: "Back" });
    expect(threadRowSlot({ ...base, mark: "unread" }, now)).toEqual({ kind: "age", age: "5m", unread: true });
    expect(threadRowSlot({ ...base, mark: "none" }, now)).toEqual({ kind: "age", age: "5m", unread: false });
    expect(threadRowSlot({ activityAt: 0, mark: "none" }, now)).toEqual({ kind: "age", age: "", unread: false });
  });

  it("gives a slim row its wake time or age, and still says a live status", () => {
    const now = new Date(2026, 9, 6, 15, 0).getTime();
    const tonight = new Date(2026, 9, 6, 18, 0).getTime();
    expect(threadShelfSlot({ mark: "none", section: "snoozed", snoozedUntil: tonight, activityAt: 1 }, now))
      .toEqual({ kind: "wake", label: "6 pm", title: snoozeWakeLabel(tonight, now) });
    expect(threadShelfSlot({ mark: "none", section: "settled", activityAt: now - 2 * 86_400_000 }, now))
      .toEqual({ kind: "age", age: "2d", unread: false });
    expect(threadShelfSlot({ mark: "needs-you", section: "settled", activityAt: 1 }, now))
      .toEqual({ kind: "status", mark: "needs-you", word: "Needs attention" });
  });

  it("words times the way a 240px row can hold them", () => {
    const now = new Date(2026, 9, 6, 15, 0).getTime(); // a Tuesday
    expect(threadWakeTime(new Date(2026, 9, 6, 18, 0).getTime(), now)).toBe("6 pm");
    expect(threadWakeTime(new Date(2026, 9, 6, 21, 30).getTime(), now)).toBe("9:30 pm");
    expect(threadWakeTime(new Date(2026, 9, 7, 9, 0).getTime(), now)).toBe("Wed");
    expect(threadWakeTime(new Date(2026, 9, 12, 9, 0).getTime(), now)).toBe("Mon");
    expect(threadWakeTime(new Date(2026, 9, 13, 0, 0).getTime(), now)).toBe("Oct 13");
    expect(threadAgeLabel(now - 20_000, now)).toBe("now");
    expect(threadAgeLabel(now - 5 * 60_000, now)).toBe("5m");
    expect(threadAgeLabel(now - 3 * 3_600_000, now)).toBe("3h");
    expect(threadAgeLabel(now - 2 * 86_400_000, now)).toBe("2d");
    expect(threadAgeLabel(now - 21 * 86_400_000, now)).toBe("3w");
    expect(threadAgeLabel(0, now)).toBe("");
    expect(threadElapsedLabel(now, now)).toBe("0:00");
    expect(threadElapsedLabel(now - 161_000, now)).toBe("2:41");
    expect(threadElapsedLabel(now - 3_729_000, now)).toBe("1:02:09");
    expect(threadElapsedLabel(now + 5_000, now)).toBe("0:00");
  });

  it("puts the pull request, else a diff that changed something, at line two's end", () => {
    expect(threadRowTrail({ pullRequest: { number: 7, state: "merged" }, diff: { added: 3, removed: 1 } }))
      .toEqual({ kind: "pr", number: 7, state: "merged" });
    expect(threadRowTrail({ diff: { added: 26, removed: 1 } })).toEqual({ kind: "diff", added: 26, removed: 1 });
    // A measured zero is silent, and unknown is never drawn as zero.
    expect(threadRowTrail({ diff: { added: 0, removed: 0 } })).toBeNull();
    expect(threadRowTrail({})).toBeNull();
  });

  it("weights the title: unread strongest, a quiet read row recedes", () => {
    expect(threadTitleTone({ unread: true, mark: "failed", selected: false })).toBe("unread");
    expect(threadTitleTone({ unread: false, mark: "none", selected: false })).toBe("read");
    expect(threadTitleTone({ unread: false, mark: "none", selected: true })).toBe("plain");
    expect(threadTitleTone({ unread: false, mark: "working", selected: false })).toBe("plain");
  });

  it("names the row by everything both lines say", () => {
    const named = { title: "Fix the flaky login test", projectLabel: "atlas", branch: "iface/flaky" };
    expect(threadRowAccessibleName(named, { kind: "age", age: "8m", unread: true },
      { kind: "pr", number: 212, state: "open" }))
      .toBe("Fix the flaky login test, Done, 8m, atlas, branch iface/flaky, pull request #212 open");
    expect(threadRowAccessibleName({ title: "Port", projectLabel: "No project" },
      { kind: "status", mark: "working", word: "Working", elapsed: "0:38" }, { kind: "diff", added: 1, removed: 2 }))
      .toBe("Port, Working, No project, 1 line added, 2 removed");
    expect(threadRowAccessibleName({ title: "Bench", projectLabel: "atlas" },
      { kind: "wake", label: "6 pm", title: "Wakes today at 6:00 PM" }))
      .toBe("Bench, Wakes today at 6:00 PM, atlas");
  });

  it("hints only the chords threadChord really reads", () => {
    const event = (keys: string) => {
      const parts = keys.split("+");
      const letter = parts.at(-1)!;
      return { key: letter, code: `Key${letter}`, metaKey: parts.includes("Meta"), ctrlKey: false,
        shiftKey: parts.includes("Shift"), altKey: parts.includes("Alt") };
    };
    expect(Object.keys(THREAD_MENU_CHORDS).sort()).toEqual(["pin", "settle", "unpin", "unsettle"]);
    for (const [id, { chord, keys, hint }] of Object.entries(THREAD_MENU_CHORDS)) {
      expect(threadChord(event(keys)), id).toBe(chord);
      expect(hint.endsWith(keys.at(-1)!), id).toBe(true);
    }
    // Every hinted entry is one the row menu offers.
    const ids = (entries: readonly { readonly id: string }[]) => entries.map(entry => entry.id);
    const open = threadMenuEntries(row({ pinned: true }), 0);
    const settled = threadMenuEntries(row({ section: "settled" }), 0);
    expect(ids(open.lead)).toEqual(expect.arrayContaining(["unpin", "settle"]));
    expect(ids(settled.lead)).toEqual(expect.arrayContaining(["pin", "unsettle"]));
  });
});

describe("folds, paging and keyboard order", () => {
  it("folds Snoozed and Settled by default and survives a corrupt preference", () => {
    expect(DEFAULT_THREAD_FOLDS)
      .toEqual({ pinned: false, active: false, snoozed: true, settled: true, fromTerminals: true });
    expect(parseThreadFolds(null)).toBe(DEFAULT_THREAD_FOLDS);
    expect(parseThreadFolds("{nope")).toBe(DEFAULT_THREAD_FOLDS);
    expect(parseThreadFolds("[]")).toBe(DEFAULT_THREAD_FOLDS);
    expect(parseThreadFolds('{"settled":false,"active":"yes"}'))
      .toEqual({ pinned: false, active: false, snoozed: true, settled: false, fromTerminals: true });
  });

  it("walks only drawn rows, and a search opens every section", () => {
    expect(threadInboxOrder(inbox(), DEFAULT_THREAD_FOLDS, SETTLED_PAGE_SIZE)).toEqual([PINNED, ACTIVE]);
    const open = { pinned: false, active: false, snoozed: false, settled: false, fromTerminals: false };
    expect(threadInboxOrder(inbox(), open, SETTLED_PAGE_SIZE)).toEqual([PINNED, ACTIVE, SNOOZED, SETTLED]);
    expect(threadInboxOrder(inbox({ query: "thread" }), DEFAULT_THREAD_FOLDS, SETTLED_PAGE_SIZE)).toEqual([]);
    expect(threadInboxOrder(inbox({ query: "release" }), DEFAULT_THREAD_FOLDS, SETTLED_PAGE_SIZE))
      .toEqual([SETTLED]);
  });

  it("pages Settled fifty at a time unless searching", () => {
    const keys = Array.from({ length: 60 }, (_, at) => `codex#settled-${String(at).padStart(2, "0")}`);
    const model = buildThreadInbox({
      sessions: keys.map(key => session(key)),
      view: view(keys.map((key, at) => thread(key, { section: "settled", settledAt: 1_000 - at }))),
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
    });
    expect(threadSectionRows(model, model.sections[0], SETTLED_PAGE_SIZE)).toHaveLength(50);
    expect(threadSectionRows(model, model.sections[0], 100)).toHaveLength(60);
    const searching = { ...model, searching: true };
    expect(threadSectionRows(searching, model.sections[0], SETTLED_PAGE_SIZE)).toHaveLength(60);
  });

  it("steps to the previous or next thread, wrapping", () => {
    const order = ["a", "b", "c"];
    expect(adjacentThreadKey(order, "b", "next")).toBe("c");
    expect(adjacentThreadKey(order, "c", "next")).toBe("a");
    expect(adjacentThreadKey(order, "a", "previous")).toBe("c");
    expect(adjacentThreadKey(order, null, "next")).toBe("a");
    expect(adjacentThreadKey(order, "gone", "previous")).toBe("c");
    expect(adjacentThreadKey([], "a", "next")).toBeNull();
  });

  it("reads the Threads chords and leaves ⌘⇧P to the palette", () => {
    const key = (over: Partial<KeyboardEvent>) => ({ key: "", code: "", metaKey: true, ctrlKey: false,
      shiftKey: false, altKey: false, ...over });
    expect(threadChord(key({ shiftKey: true, key: "{", code: "BracketLeft" }))).toBe("previous");
    expect(threadChord(key({ shiftKey: true, key: "}", code: "BracketRight" }))).toBe("next");
    expect(threadChord(key({ shiftKey: true, key: "S", code: "KeyS" }))).toBe("settle");
    expect(threadChord(key({ altKey: true, key: "π", code: "KeyP" }))).toBe("pin");
    expect(threadChord(key({ shiftKey: true, key: "P", code: "KeyP" }))).toBeNull();
    expect(threadChord(key({ key: "z", code: "KeyZ" }))).toBe("undo");
    expect(threadChord(key({ shiftKey: true, key: "z", code: "KeyZ" }))).toBeNull();
    expect(threadChord(key({ metaKey: false, ctrlKey: true, shiftKey: true, code: "BracketRight" }))).toBe("next");
    expect(threadChord(key({ metaKey: false, shiftKey: true, code: "BracketRight" }))).toBeNull();
  });
});

describe("the project scope", () => {
  it("parses and round-trips the stored scope", () => {
    expect(parseThreadScope(null)).toBe(ALL_THREAD_PROJECTS);
    expect(parseThreadScope("project:")).toBe(ALL_THREAD_PROJECTS);
    expect(parseThreadScope("garbage")).toBe(ALL_THREAD_PROJECTS);
    expect(parseThreadScope("none")).toBe(NO_THREAD_PROJECT);
    expect(parseThreadScope(threadScopeFor(ATLAS))).toBe(`project:${ATLAS}`);
    expect(threadScopeRoot(threadScopeFor(ATLAS))).toBe(ATLAS);
    expect(threadScopeRoot(NO_THREAD_PROJECT)).toBeNull();
    expect(threadScopeRoot(ALL_THREAD_PROJECTS)).toBeUndefined();
    expect(threadScopeFor(null)).toBe(NO_THREAD_PROJECT);
  });

  it("offers All projects, each project, then No project; twins carry their path", () => {
    expect(threadScopeOptions({ projects: [{ root: "/a/web", name: "web" }, { root: "/b/web", name: "web" },
      { root: "/c/api", name: "api" }], hasNoProject: true })).toEqual([
      { value: "all", label: "All projects" },
      { value: "project:/a/web", label: "web", description: "/a/web" },
      { value: "project:/b/web", label: "web", description: "/b/web" },
      { value: "project:/c/api", label: "api" },
      { value: "none", label: "No project" },
    ]);
    expect(threadScopeOptions({ projects: [], hasNoProject: false })).toEqual([{ value: "all", label: "All projects" }]);
  });
});

describe("snooze presets", () => {
  it("offers an hour, tonight at 6 pm while ahead, tomorrow 9 am and next Monday 9 am", () => {
    // Tuesday 6 October 2026, 10:00 local.
    const now = new Date(2026, 9, 6, 10, 0).getTime();
    expect(snoozePresets(now)).toEqual([
      { id: "hour", label: "1 hour", until: now + 3_600_000 },
      { id: "tonight", label: "Tonight 6 pm", until: new Date(2026, 9, 6, 18, 0).getTime() },
      { id: "tomorrow", label: "Tomorrow 9 am", until: new Date(2026, 9, 7, 9, 0).getTime() },
      { id: "monday", label: "Next Monday 9 am", until: new Date(2026, 9, 12, 9, 0).getTime() },
    ]);
  });

  it("drops Tonight once 6 pm has passed, and a Monday snoozes a week out", () => {
    const mondayEvening = new Date(2026, 9, 12, 19, 30).getTime();
    expect(snoozePresets(mondayEvening).map(preset => preset.id)).toEqual(["hour", "tomorrow", "monday"]);
    expect(snoozePresets(mondayEvening)[2].until).toBe(new Date(2026, 9, 19, 9, 0).getTime());
    const sunday = new Date(2026, 9, 11, 8, 0).getTime();
    expect(snoozePresets(sunday).at(-1)!.until).toBe(new Date(2026, 9, 12, 9, 0).getTime());
  });
});

describe("the row menu's thread commands", () => {
  const now = new Date(2026, 9, 6, 10, 0).getTime();
  const ids = (entries: readonly (ThreadMenuCommand | ThreadMenuSubmenu)[]) => entries.map(entry => entry.id);

  it("offers Pin, Settle, Snooze ▸ and Mark unread on an active thread", () => {
    const { lead, tail } = threadMenuEntries(row({ key: "codex#a" }), now);
    expect(ids(lead)).toEqual(["pin", "settle", "snooze", "markUnread"]);
    expect(ids(tail)).toEqual(["regenerateTitle", "autoSettle"]);
    expect((lead[0] as ThreadMenuCommand).commands).toEqual([{ kind: "pin", key: "codex#a" }]);
    expect(lead[1]).toMatchObject({ commands: [{ kind: "settle", key: "codex#a" }], undo: "settle" });
    expect((lead[3] as ThreadMenuCommand).commands).toEqual([{ kind: "markUnread", key: "codex#a" }]);
    const snooze = lead[2] as ThreadMenuSubmenu;
    expect(snooze.items.map(item => item.label))
      .toEqual(["1 hour", "Tonight 6 pm", "Tomorrow 9 am", "Next Monday 9 am"]);
    expect(snooze.items[0]).toMatchObject({ undo: "snooze",
      commands: [{ kind: "snooze", key: "codex#a", until: now + 3_600_000 }] });
    expect((tail[0] as ThreadMenuCommand).commands).toEqual([{ kind: "regenerateTitle", key: "codex#a" }]);
    expect((tail[1] as ThreadMenuSubmenu).items).toEqual([
      { id: "autoSettle-on", label: "Enabled", checked: true,
        commands: [{ kind: "setAutoSettle", key: "codex#a", enabled: true }] },
      { id: "autoSettle-off", label: "Disabled", checked: false,
        commands: [{ kind: "setAutoSettle", key: "codex#a", enabled: false }] },
    ]);
  });

  it("offers Unpin, Wake now and no Mark unread where they apply", () => {
    const snoozed = threadMenuEntries(row({ section: "snoozed", pinned: true, unread: true }), now);
    expect(ids(snoozed.lead)).toEqual(["unpin", "settle", "snooze"]);
    expect((snoozed.lead[2] as ThreadMenuSubmenu).items.at(-1))
      .toEqual({ id: "unsnooze", label: "Wake now", commands: [{ kind: "unsnooze", key: "codex#a" }] });
  });

  it("offers Refresh pull request only where Rust tracks one", () => {
    const tracked = threadMenuEntries(row({ key: "codex#wt" }), now, { refreshPullRequest: true });
    expect(ids(tracked.tail)).toEqual(["regenerateTitle", "refreshPullRequest", "autoSettle"]);
    expect((tracked.tail[1] as ThreadMenuCommand).commands)
      .toEqual([{ kind: "refreshPullRequest", key: "codex#wt" }]);
    const settings = { trackPullRequests: true };
    const worktree = thread("codex#wt", { workspace: { mode: "worktree", projectRoot: ATLAS,
      worktreePath: "/w/.interface-worktrees/x", branch: "iface/x", owned: true } });
    expect(threadTracksPullRequest(worktree, settings)).toBe(true);
    expect(threadTracksPullRequest(worktree, { trackPullRequests: false })).toBe(false);
    expect(threadTracksPullRequest(worktree, null)).toBe(false);
    expect(threadTracksPullRequest({ ...worktree, section: "settled" }, settings)).toBe(false);
    expect(threadTracksPullRequest(thread("codex#wt", { workspace: { mode: "worktree", projectRoot: ATLAS,
      owned: true } }), settings)).toBe(false);
    expect(threadTracksPullRequest(thread("codex#l", { workspace: { mode: "local", projectRoot: ATLAS,
      owned: false } }), settings)).toBe(false);
    expect(threadTracksPullRequest(undefined, settings)).toBe(false);
  });

  it("names the pull request on the hover card", () => {
    const fixtureThread = VIEW.threads.find(item => item.pullRequest !== undefined);
    expect(threadPullRequestFact(fixtureThread)).toBe("#482 · open");
    expect(threadPullRequestFact(thread("codex#a"))).toBeUndefined();
    expect(threadPullRequestFact(undefined)).toBeUndefined();
  });

  it("offers Pin and Un-settle and no Snooze on a settled thread", () => {
    const { lead } = threadMenuEntries(row({ section: "settled", autoSettle: false }), now);
    expect(ids(lead)).toEqual(["pin", "unsettle", "markUnread"]);
    expect((lead[1] as ThreadMenuCommand).commands).toEqual([{ kind: "unsettle", key: "codex#a" }]);
    expect((lead[1] as ThreadMenuCommand).undo).toBeUndefined();
  });

  it("reorders from the keyboard: Move up and Move down in Pinned and Active", () => {
    // Review, 6 Oct 2026: reordering was drag-only.
    const keys = ["codex#1", "codex#2", "codex#3", "codex#4"];
    const middle = threadMenuEntries(row({ key: "codex#2" }), now, { sectionKeys: keys });
    expect(ids(middle.lead)).toEqual(["pin", "settle", "snooze", "markUnread", "moveUp", "moveDown"]);
    expect(middle.lead.slice(4)).toEqual([
      { id: "moveUp", label: "Move up",
        commands: [{ kind: "reorder", key: "codex#2", section: "active", beforeKey: "codex#1" }] },
      { id: "moveDown", label: "Move down",
        commands: [{ kind: "reorder", key: "codex#2", section: "active", beforeKey: "codex#4" }] },
    ]);
    // Below the second-to-last row is the end of the section.
    expect(threadMoveEntries({ key: "codex#3", section: "pinned" }, keys))
      .toEqual([
        { id: "moveUp", label: "Move up",
          commands: [{ kind: "reorder", key: "codex#3", section: "pinned", beforeKey: "codex#2" }] },
        { id: "moveDown", label: "Move down",
          commands: [{ kind: "reorder", key: "codex#3", section: "pinned" }] },
      ]);
    // Only the moves there is room for; none in Snoozed or Settled.
    expect(threadMoveEntries({ key: "codex#1", section: "active" }, keys).map(entry => entry.id)).toEqual(["moveDown"]);
    expect(threadMoveEntries({ key: "codex#4", section: "active" }, keys).map(entry => entry.id)).toEqual(["moveUp"]);
    expect(threadMoveEntries({ key: "codex#1", section: "active" }, ["codex#1"])).toEqual([]);
    expect(threadMoveEntries({ key: "codex#2", section: "settled" }, keys)).toEqual([]);
    expect(threadMoveEntries({ key: "codex#2", section: "snoozed" }, keys)).toEqual([]);
  });

  it("moves focus to the next row, else the previous, when a row leaves", () => {
    const order = ["a", "b", "c"];
    expect(threadFocusAfterRemoval(order, "b")).toBe("c");
    expect(threadFocusAfterRemoval(order, "c")).toBe("b");
    expect(threadFocusAfterRemoval(["a"], "a")).toBeNull();
    expect(threadFocusAfterRemoval(order, "x")).toBeNull();
  });

  it("knows which undoable actions take a row out of its section (e2e 6 Oct: ⌘Z lost focus after a settle)", () => {
    expect(threadActionLeavesSection("archive", "pinned")).toBe(true);
    expect(threadActionLeavesSection("settle", "active")).toBe(true);
    expect(threadActionLeavesSection("settle", "pinned")).toBe(true);
    expect(threadActionLeavesSection("snooze", "active")).toBe(true);
    // A snoozed thread snoozed again stays where it is, focus with it.
    expect(threadActionLeavesSection("snooze", "snoozed")).toBe(false);
    expect(threadActionLeavesSection("settle", "settled")).toBe(false);
    expect(threadActionLeavesSection(undefined, "active")).toBe(false);
  });
});

describe("drag and drop", () => {
  const active = row({ key: "codex#a", section: "active" });
  it("reorders within Pinned and Active", () => {
    expect(threadDropCommands(active, { section: "active", beforeKey: "codex#b" }))
      .toEqual([{ kind: "reorder", key: "codex#a", section: "active", beforeKey: "codex#b" }]);
    expect(threadDropCommands(active, { section: "active" }))
      .toEqual([{ kind: "reorder", key: "codex#a", section: "active" }]);
    const pinned = row({ key: "codex#p", section: "pinned", pinned: true });
    expect(threadDropCommands(pinned, { section: "pinned", beforeKey: "codex#q" }))
      .toEqual([{ kind: "reorder", key: "codex#p", section: "pinned", beforeKey: "codex#q" }]);
  });

  it("pins, unpins, settles and wakes across sections", () => {
    expect(threadDropCommands(active, { section: "pinned", beforeKey: "codex#p" })).toEqual([
      { kind: "pin", key: "codex#a" },
      { kind: "reorder", key: "codex#a", section: "pinned", beforeKey: "codex#p" },
    ]);
    expect(threadDropCommands(row({ key: "codex#p", section: "pinned", pinned: true }), { section: "active" }))
      .toEqual([{ kind: "unpin", key: "codex#p" }, { kind: "reorder", key: "codex#p", section: "active" }]);
    expect(threadDropCommands(active, { section: "settled" })).toEqual([{ kind: "settle", key: "codex#a" }]);
    const snoozed = row({ key: "codex#s", section: "snoozed" });
    expect(threadDropCommands(snoozed, { section: "active", beforeKey: "codex#a" })).toEqual([
      { kind: "unsnooze", key: "codex#s" },
      { kind: "reorder", key: "codex#s", section: "active", beforeKey: "codex#a" },
    ]);
    // Snoozing keeps the pin: waking a pinned snooze into Pinned needs no pin.
    expect(threadDropCommands(row({ key: "codex#s", section: "snoozed", pinned: true }), { section: "pinned" }))
      .toEqual([{ kind: "unsnooze", key: "codex#s" }, { kind: "reorder", key: "codex#s", section: "pinned" }]);
    const settled = row({ key: "codex#t", section: "settled" });
    expect(threadDropCommands(settled, { section: "active" }))
      .toEqual([{ kind: "unsettle", key: "codex#t" }, { kind: "reorder", key: "codex#t", section: "active" }]);
    expect(threadDropCommands(settled, { section: "pinned" }))
      .toEqual([{ kind: "pin", key: "codex#t" }, { kind: "reorder", key: "codex#t", section: "pinned" }]);
  });

  it("sends nothing for a drop onto itself, into Snoozed, or Settled onto Settled", () => {
    expect(threadDropCommands(active, { section: "active", beforeKey: "codex#a" })).toEqual([]);
    expect(threadDropCommands(active, { section: "snoozed" })).toEqual([]);
    expect(threadDropCommands(row({ section: "settled" }), { section: "settled" })).toEqual([]);
  });

  it("offers Undo for a drop that settles", () => {
    expect(threadDropUndo([{ kind: "settle", key: "codex#a" }])).toBe("settle");
    expect(threadDropUndo([{ kind: "pin", key: "codex#a" }])).toBeUndefined();
  });
});

describe("Undo", () => {
  const now = 1_000;
  const model = buildThreadInbox({
    sessions: ["codex#a", "codex#b", "codex#c"].map(key => session(key)),
    view: view([
      thread("codex#a", { activeOrder: "a0" }),
      thread("codex#b", { activeOrder: "a1" }),
      thread("codex#c", { section: "pinned", pinnedAt: 1, pinOrder: "a0" }),
    ]),
    scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
  });
  const find = (key: string) => model.sections.flatMap(section => section.rows).find(item => item.key === key)!;

  it("un-settles a settled active thread back to its place", () => {
    const undo = threadUndo("settle", find("codex#a"), model);
    expect(undo).toMatchObject({ section: "active", next: "codex#b", pinned: false });
    expect(threadUndoCommands(undo, now)).toEqual([
      { kind: "unsettle", key: "codex#a" },
      { kind: "reorder", key: "codex#a", section: "active", beforeKey: "codex#b" },
    ]);
    expect(threadUndoCommands(threadUndo("settle", find("codex#b"), model), now)).toEqual([
      { kind: "unsettle", key: "codex#b" },
      { kind: "reorder", key: "codex#b", section: "active" },
    ]);
    expect(threadUndoMessage(undo)).toBe("Settled “Thread a”.");
  });

  it("re-pins a settled pinned thread, and restores a snooze still ahead", () => {
    expect(threadUndoCommands(threadUndo("settle", find("codex#c"), model), now)).toEqual([
      { kind: "pin", key: "codex#c" },
      { kind: "reorder", key: "codex#c", section: "pinned" },
    ]);
    const snoozedPinned = threadUndo("settle", row({ key: "codex#s", section: "snoozed", pinned: true,
      snoozedUntil: 5_000 }), model);
    expect(threadUndoCommands(snoozedPinned, now))
      .toEqual([{ kind: "pin", key: "codex#s" }, { kind: "snooze", key: "codex#s", until: 5_000 }]);
    const snoozedPast = threadUndo("settle", row({ key: "codex#s", section: "snoozed", snoozedUntil: 500 }), model);
    expect(threadUndoCommands(snoozedPast, now)).toEqual([{ kind: "unsettle", key: "codex#s" }]);
  });

  it("wakes a fresh snooze, or puts back the snooze it replaced", () => {
    expect(threadUndoCommands(threadUndo("snooze", find("codex#a"), model), now))
      .toEqual([{ kind: "unsnooze", key: "codex#a" }]);
    const resnoozed = threadUndo("snooze", row({ key: "codex#s", section: "snoozed", snoozedUntil: 9_000 }), model);
    expect(threadUndoCommands(resnoozed, now)).toEqual([{ kind: "snooze", key: "codex#s", until: 9_000 }]);
    expect(threadUndoMessage(resnoozed)).toBe("Snoozed “A”.");
  });

  it("leaves Archive to the session's own unarchive", () => {
    const undo = threadUndo("archive", find("codex#a"), model);
    expect(threadUndoCommands(undo, now)).toEqual([]);
    expect(threadUndoMessage(undo)).toBe("Archived “Thread a”.");
  });
});

describe("run ends", () => {
  const running = view([thread("codex#a", { lastRun: { startedAt: 1 } }), thread("codex#b")]);
  const done = view([thread("codex#a", { lastRun: { startedAt: 1, endedAt: 9, outcome: "completed" } }),
    thread("codex#b")]);
  it("counts a run end this window saw happen, never one it found already ended", () => {
    const first = threadRunEnds(null, done);
    expect(first.ended).toEqual([]);
    const watching = threadRunEnds(null, running);
    expect(watching.ended).toEqual([]);
    const after = threadRunEnds(watching.seen, done);
    expect(after.ended).toEqual(["codex#a"]);
    expect(threadRunEnds(after.seen, done).ended).toEqual([]);
    // A thread new to the view whose run already ended was seen ending too.
    expect(threadRunEnds(new Map(), done).ended).toEqual(["codex#a"]);
  });
});

describe("kept worktrees", () => {
  const WT = "/Users/owner/Developer/Projects/.interface-worktrees/fix-login-3f9a2c";
  const owned = (kept?: "dirty" | "ignored" | "inUse") => thread("codex#a", {
    workspace: { mode: "worktree", projectRoot: ATLAS, worktreePath: WT, owned: true,
      ...(kept === undefined ? {} : { kept }) },
  });
  it("names a worktree an archive or delete kept once, with why, and never one already kept", () => {
    // Opening the app on a kept worktree repeats nothing.
    expect(threadKeptWorktrees(null, view([owned("ignored")])).kept).toEqual([]);
    const before = threadKeptWorktrees(null, view([owned(), thread("codex#b")]));
    const archived = threadKeptWorktrees(before.seen, view([owned("ignored"), thread("codex#b")]));
    expect(archived.kept).toEqual([{ key: "codex#a", path: WT, kept: "ignored" }]);
    expect(threadKeptWorktrees(archived.seen, view([owned("ignored")])).kept).toEqual([]);
    // Unarchived (cleared), then archived again with uncommitted changes.
    const cleared = threadKeptWorktrees(archived.seen, view([owned()]));
    expect(threadKeptWorktrees(cleared.seen, view([owned("dirty")])).kept)
      .toEqual([{ key: "codex#a", path: WT, kept: "dirty" }]);
  });

  it("says why it stayed and that the owner removes it", () => {
    expect(threadKeptText({ path: WT, kept: "ignored" })).toBe(
      `Kept the worktree ${WT}: it has ignored files such as .env. Remove it when you’re done.`);
    expect(threadKeptText({ path: WT, kept: "dirty" })).toBe(
      `Kept the worktree ${WT}: it has uncommitted changes. Remove it when you’re done.`);
    expect(threadKeptText({ path: WT, kept: "inUse" })).toBe(
      `Kept the worktree ${WT}: another session uses it. Remove it when you’re done.`);
  });
});

describe("a row that needs you", () => {
  // Owner, 2026-10-07, "Inbox under load": it floats to the top of Pinned and
  // Active, display only. Every other row keeps Rust's order (a working row
  // never moves), and every command still means a place in Rust's order.
  const model = buildThreadInbox({
    sessions: [
      session("codex#a"), session("codex#b", { status: "working" }), session("codex#c", { status: "waiting" }),
      session("codex#d"), session("codex#p1"), session("codex#p2", { status: "waiting" }),
      session("codex#s1"), session("codex#s2", { status: "waiting" }),
    ],
    view: view([
      thread("codex#a", { activeOrder: "a0" }), thread("codex#b", { activeOrder: "a1" }),
      thread("codex#c", { activeOrder: "a2" }), thread("codex#d", { activeOrder: "a3" }),
      thread("codex#p1", { section: "pinned", pinnedAt: 1, pinOrder: "a0" }),
      thread("codex#p2", { section: "pinned", pinnedAt: 1, pinOrder: "a1" }),
      thread("codex#s1", { section: "settled", settledAt: 20 }), thread("codex#s2", { section: "settled", settledAt: 10 }),
    ]),
    scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
  });
  const section = (name: string) => model.sections.find(item => item.section === name)!;
  const keys = (rows: readonly ThreadInboxRow[]) => rows.map(item => item.key);

  it("floats to the top of its section as drawn, while the working row stays in Rust's place", () => {
    expect(keys(section("active").rows)).toEqual(["codex#a", "codex#b", "codex#c", "codex#d"]);
    expect(keys(section("active").drawn)).toEqual(["codex#c", "codex#a", "codex#b", "codex#d"]);
    expect(keys(section("pinned").drawn)).toEqual(["codex#p2", "codex#p1"]);
    // The shelves keep their own order.
    expect(keys(section("settled").drawn)).toEqual(["codex#s1", "codex#s2"]);
    // What draws, and what ⌘⇧[ / ⌘⇧] walk, is the drawn order.
    expect(keys(threadSectionRows(model, section("active"), SETTLED_PAGE_SIZE)))
      .toEqual(["codex#c", "codex#a", "codex#b", "codex#d"]);
    expect(threadInboxOrder(model, { pinned: false, active: false, snoozed: false, settled: true,
      fromTerminals: true }, SETTLED_PAGE_SIZE))
      .toEqual(["codex#p2", "codex#p1", "codex#c", "codex#a", "codex#b", "codex#d"]);
    // Nothing to float, or already on top: the very same array.
    const plain = section("active").rows.filter(item => item.mark !== "needs-you");
    expect(threadNeedsYouFirst(plain)).toBe(plain);
    const top = section("pinned").drawn;
    expect(threadNeedsYouFirst(top)).toBe(top);
  });

  it("is moved, dropped beside and put back by Rust's order, not by where it is drawn", () => {
    const c = section("active").rows.find(item => item.key === "codex#c")!;
    const { lead } = threadMenuEntries(c, 0, { sectionKeys: keys(section("active").rows) });
    const moves = lead.filter((entry): entry is ThreadMenuCommand => entry.id === "moveUp" || entry.id === "moveDown");
    // Drawn first, yet it can move up: Rust keeps it third.
    expect(moves.map(entry => entry.commands)).toEqual([
      [{ kind: "reorder", key: "codex#c", section: "active", beforeKey: "codex#b" }],
      [{ kind: "reorder", key: "codex#c", section: "active" }],
    ]);
    // Undo puts it back before the row Rust keeps after it, not the one drawn under it.
    expect(threadUndo("settle", c, model).next).toBe("codex#d");
    // A drop on the lower half of the floated row lands before Rust's next row.
    const rust = keys(section("active").rows);
    expect(threadDropBeforeKey(rust, "codex#c", true)).toBe("codex#c");
    expect(threadDropBeforeKey(rust, "codex#c", false)).toBe("codex#d");
    expect(threadDropBeforeKey(rust, "codex#d", false)).toBeUndefined();
    expect(threadDropBeforeKey(rust, "codex#gone", false)).toBeUndefined();
  });
});

describe("a live thread's second line", () => {
  const ROOT = "/Users/ann/Developer/atlas-web";
  const call = (callId: string, tool: string, input: unknown, agentKey?: string): EngineEvent => ({
    kind: "tool.call", callId, tool, summary: tool, input, ...(agentKey === undefined ? {} : { agentKey }) });
  const result = (callId: string): EngineEvent => ({ kind: "tool.result", callId, ok: true, summary: "" });
  const status = (next: SessionStatus): EngineEvent => ({ kind: "status.changed", status: next });
  const approval = (tool: string, input: unknown): EngineEvent => ({
    kind: "approval.requested", approvalId: "ap-1", tool, summary: tool, input });

  it("reads the newest open step, the main thread's before a subagent's", () => {
    const edit = call("1", "Edit", { file_path: `${ROOT}/src/routes/login.ts` });
    expect(threadLiveFacts([status("working"), edit])).toEqual({ running: true,
      step: { tool: "Edit", summary: "Edit", input: { file_path: `${ROOT}/src/routes/login.ts` } } });
    // A finished call is no step: the turn is thinking again.
    expect(threadLiveFacts([status("working"), edit, result("1")])).toEqual({ running: true });
    // A subagent's calls give way to the open Task call that spawned it, and
    // speak only once no main-thread call is open.
    const task = call("t", "Task", { description: "Find callers of verify" });
    const child = call("k", "Grep", { pattern: "verify" }, "t");
    expect(threadLiveFacts([status("working"), task, child])?.step?.tool).toBe("Task");
    expect(threadLiveFacts([status("working"), child])?.step?.tool).toBe("Grep");
    // A delta alone is working, by the registry's rule; a thinking pulse too.
    expect(threadLiveFacts([{ kind: "message.delta", text: "Looking" }])?.running).toBe(true);
    expect(threadLiveFacts([{ kind: "thinking.signal", active: true }])?.running).toBe(true);
  });

  it("has nothing to say for an empty log, and nothing running after the turn ends", () => {
    expect(threadLiveFacts([])).toBeNull();
    expect(threadLiveFacts([status("working"), call("1", "Bash", { command: "ls" }), status("idle")]))
      .toEqual({ running: false });
    expect(threadLiveFacts([status("working"), call("1", "Bash", { command: "ls" }), { kind: "session.closed" }]))
      .toEqual({ running: false });
  });

  it("parks on the newest approval until the status moves on, and asks a question in its own words", () => {
    const bash = approval("Bash", { command: "npm test" });
    expect(threadLiveFacts([status("working"), bash])).toEqual({ running: false,
      ask: { tool: "Bash", summary: "Bash", input: { command: "npm test" } } });
    // Deltas while it waits do not move it on (Rust's rule); an answer's status change does.
    expect(threadLiveFacts([bash, { kind: "message.delta", text: "…" }])?.ask?.tool).toBe("Bash");
    expect(threadLiveFacts([bash, status("working")])?.ask).toBeUndefined();
    expect(threadLiveFacts([bash, { kind: "session.closed" }])?.ask).toBeUndefined();
    const question = approval("AskUserQuestion", { questions: [{ question: "Which database?", header: "DB",
      options: [{ label: "Postgres", description: "" }, { label: "SQLite", description: "" }], multiSelect: false }] });
    expect(threadLiveFacts([question])?.ask?.question).toBe("Which database?");
  });

  it("keeps the newest failure's first line through the close, until a later turn works", () => {
    const failed: EngineEvent[] = [status("working"),
      { kind: "error", message: "\nRate limit reached for gpt-5\nRetry after 20s", retryable: false },
      { kind: "session.closed" }];
    expect(threadLiveFacts(failed)).toEqual({ running: false, error: "Rate limit reached for gpt-5" });
    // A retryable error is a delay, not a failure.
    expect(threadLiveFacts([status("working"), { kind: "error", message: "Overloaded", retryable: true }])?.error)
      .toBeUndefined();
    expect(threadLiveFacts([...failed, status("working")])?.error).toBeUndefined();
  });

  it("keeps a reading by identity when nothing it says has changed", () => {
    const input = { command: "npm test" };
    const a: ThreadLiveFacts = { running: true, step: { tool: "Bash", summary: "npm test", input } };
    expect(sameThreadLiveFacts(a, { running: true, step: { tool: "Bash", summary: "npm test", input } })).toBe(true);
    expect(sameThreadLiveFacts(a, { running: true, step: { tool: "Bash", summary: "npm test", input: { ...input } } })).toBe(false);
    expect(sameThreadLiveFacts(a, { running: true })).toBe(false);
    expect(sameThreadLiveFacts({ running: false, error: "x" }, { running: false, error: "y" })).toBe(false);
    expect(sameThreadLiveFacts({ running: false, ask: { tool: "Q", summary: "", input, question: "A?" } },
      { running: false, ask: { tool: "Q", summary: "", input, question: "B?" } })).toBe(false);
  });

  it("words the line from the row's one mark: the step, Thinking, the ask, the error, else the project line", () => {
    const at = (mark: ThreadInboxRow["mark"]) => ({ mark, sessionRoot: ROOT });
    const editing: ThreadLiveFacts = { running: true,
      step: { tool: "Edit", summary: `${ROOT}/src/routes/login.ts`, input: { file_path: `${ROOT}/src/routes/login.ts` } } };
    expect(threadRowLine(at("working"), editing)).toEqual({ kind: "step", text: "Editing login.ts" });
    expect(threadRowLine(at("working"), { running: true,
      step: { tool: "Bash", summary: "", input: { command: "cargo test -p interface-voice" } } }))
      .toEqual({ kind: "step", text: "Running cargo test -p interface-voice" });
    expect(threadRowLine(at("working"), { running: true })).toEqual({ kind: "step", text: "Thinking" });
    // The log does not see the run the listing reports: the project line.
    expect(threadRowLine(at("working"), { running: false })).toBe(THREAD_META_LINE);
    expect(threadRowLine(at("needs-you"), { running: false,
      ask: { tool: "Bash", summary: "Run the tests", input: { command: "npm test" } } }))
      .toEqual({ kind: "ask", text: "Run npm test?" });
    expect(threadRowLine(at("needs-you"), { running: false,
      ask: { tool: "AskUserQuestion", summary: "", input: {}, question: "Which database?" } }))
      .toEqual({ kind: "ask", text: "Which database?" });
    expect(threadRowLine(at("needs-you"), { running: false })).toBe(THREAD_META_LINE);
    expect(threadRowLine(at("failed"), { running: false, error: "Rate limit reached" }))
      .toEqual({ kind: "error", text: "Rate limit reached" });
    expect(threadRowLine(at("failed"), { running: false })).toBe(THREAD_META_LINE);
    // Only the live marks speak, and nothing without a live log.
    expect(threadRowLine(at("unread"), editing)).toBe(THREAD_META_LINE);
    expect(threadRowLine(at("none"), { running: false, error: "x" })).toBe(THREAD_META_LINE);
    expect(threadRowLine(at("working"), undefined)).toBe(THREAD_META_LINE);
  });

  it("shows a step's paths relative to where the session runs, a worktree's included", () => {
    const model = inbox();
    const active = model.sections[1].rows[0];
    expect(active.sessionRoot).toBe("/Users/owner/Developer/Projects/.interface-worktrees/fix-the-flaky-login-test-3f9a2c");
    expect(model.sections[2].rows[0].sessionRoot).toBe("/tmp/private");
    expect(threadRowLine({ ...active, mark: "working" }, { running: true, step: { tool: "Read", summary: "",
      input: { file_path: `${active.sessionRoot}/src/a.ts` } } })).toEqual({ kind: "step", text: "Reading a.ts" });
  });

  it("names a row by what it asks or why it failed, never by the step it is on", () => {
    const named = { title: "Migrate", projectLabel: "atlas" };
    const slot = { kind: "status", mark: "needs-you", word: "Needs you" } as const;
    expect(threadRowAccessibleName(named, slot, null, { kind: "ask", text: "Run npm test?" }))
      .toBe("Migrate, Needs you, Run npm test?, atlas");
    expect(threadRowAccessibleName(named, { kind: "status", mark: "working", word: "Working", elapsed: "0:09" }, null,
      { kind: "step", text: "Editing login.ts" })).toBe("Migrate, Working, atlas");
  });
});

describe("the bridges to Code and the Agent tab (review C1)", () => {
  const V42 = fixtureV42.view as ThreadsView;
  const FROM_TERMINAL = "claude#5b0c1d2e-3f40-4a51-8b62-c7d8e9f0a1b2";
  const LOCAL = "codex#0199f2a4-7c1e-7d30-9a5b-1c2d3e4f5a6b";

  it("draws Rust's From terminals as the last shelf, folded, offering Continue as thread", () => {
    expect(threadDrawnSection("fromTerminals")).toBe("fromTerminals");
    const model = buildThreadInbox({
      sessions: [session(FROM_TERMINAL, { origin: "store", kind: "code" }), session(LOCAL, { kind: "code" })],
      view: V42, scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
    });
    expect(keysBySection(model)).toEqual({ active: [LOCAL], fromTerminals: [FROM_TERMINAL] });
    expect(model.sections.at(-1)?.label).toBe("From terminals");
    expect(DEFAULT_THREAD_FOLDS.fromTerminals).toBe(true);
    const fromTerminal = model.sections.at(-1)!.rows[0]!;
    const { lead } = threadMenuEntries(fromTerminal, 0);
    expect(lead.map(entry => entry.id)).toContain("continueFromTerminal");
    expect(lead.map(entry => entry.id)).not.toContain("snooze");
    const entry = lead.find(item => item.id === "continueFromTerminal") as ThreadMenuCommand;
    expect(entry.label).toBe("Continue as thread");
    expect(entry.commands).toEqual([{ kind: "continueFromTerminal", key: FROM_TERMINAL }]);
  });

  it("offers Open in terminal for an idle Claude or Codex thread and says why not otherwise", () => {
    expect(threadTerminalRefusal({ engine: "claude", status: "idle" })).toBeNull();
    expect(threadTerminalRefusal({ engine: "codex", status: "failed" })).toBeNull();
    expect(threadTerminalRefusal({ engine: "claude", status: "working" })).toContain("Stop the turn");
    expect(threadTerminalRefusal({ engine: "codex", status: "waiting" })).toContain("Stop the turn");
    expect(threadTerminalRefusal({ engine: "acp:grok" as SessionSummary["engine"], status: "idle" }))
      .toContain("Only Claude and Codex");
  });

  it("hands a teammate the thread's title, where it runs and how to find it, never its session", () => {
    const task = threadHandoffTask({
      title: "Fix the flaky login test", key: LOCAL, projectRoot: ATLAS, sessionRoot: `${ATLAS}-wt`,
      branch: "iface/fix-login",
    });
    expect(task).toContain('"Fix the flaky login test"');
    expect(task).toContain(`in ${ATLAS}-wt (branch iface/fix-login)`);
    expect(task).toContain(LOCAL);
    expect(task).toContain("do not resume or edit that session");
    expect(threadHandoffTask({ title: "Ideas", key: "claude#x", projectRoot: null, sessionRoot: "/tmp" }))
      .toContain("with no project folder");
    expect(threadHandoffTask({
      title: "Ideas", key: "claude#x", projectRoot: null, sessionRoot: "/tmp", summary: "Two left.",
    })).toContain("Where it stands:\nTwo left.");
  });

  it("pages From terminals independently of Settled", () => {
    const settledKeys = Array.from({ length: 60 }, (_, at) => `codex#s-${String(at).padStart(2, "0")}`);
    const terminalKeys = Array.from({ length: 60 }, (_, at) => `claude#t-${String(at).padStart(2, "0")}`);
    const model = buildThreadInbox({
      sessions: [...settledKeys, ...terminalKeys].map(key => session(key)),
      view: view([
        ...settledKeys.map((key, at) => thread(key, { section: "settled", settledAt: 1_000 - at })),
        ...terminalKeys.map(key => thread(key, { section: "fromTerminals" })),
      ]),
      scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
    });
    const settledSection = model.sections.find(entry => entry.section === "settled")!;
    const fromTerminalsSection = model.sections.find(entry => entry.section === "fromTerminals")!;
    expect(threadSectionRows(model, settledSection, 50, 100)).toHaveLength(50);
    expect(threadSectionRows(model, fromTerminalsSection, 50, 100)).toHaveLength(60);
    expect(threadSectionRows(model, fromTerminalsSection, 100, 50)).toHaveLength(50);
  });
});

describe("a thread's tile (the TN Code look, owner 2026-11-16)", () => {
  it("shows the first letters of the title's first two words, in capitals", () => {
    expect(threadInitials("Replay: showcase")).toBe("RS");
    expect(threadInitials("fix login redirect")).toBe("FL");
    expect(threadInitials("  \u201cquoted\u201d title")).toBe("QT");
    expect(threadInitials("3 flaky tests")).toBe("3F");
    expect(threadInitials("\u00e9t\u00e9 \u00e0 Paris")).toBe("\u00c9\u00c0");
  });

  it("shows two letters of a one-word title, and nothing for a title with no letters", () => {
    expect(threadInitials("Refactor")).toBe("RE");
    expect(threadInitials("x")).toBe("X");
    expect(threadInitials("")).toBe("");
    expect(threadInitials("\u2014 \u2026")).toBe("");
  });
});
