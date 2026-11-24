import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ThreadCommand, ThreadView, ThreadsView } from "../shared/threads";
import { shellPaletteCommands } from "./palette-commands";
import { threadFocusVisit, threadRunEndVisit } from "./thread-inbox";
import { sendThreadCommands, THREAD_SCOPE_KEY } from "./use-thread-inbox";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const hook = readFileSync(new URL("./use-thread-inbox.ts", import.meta.url), "utf8");

function view(threads: Partial<ThreadView>[]): ThreadsView {
  return {
    version: 1, revision: 1, now: 0,
    settings: { autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true },
    threads: threads.map(thread => ({ key: "codex#a", section: "active", createdAt: 0, woke: false, unread: false,
      autoSettle: true, turns: [], ...thread })),
  };
}

describe("sending a thread action", () => {
  it("sends the commands in order and stops at the first refusal, reporting it once", async () => {
    const asked: ThreadCommand[] = [];
    const errors: unknown[] = [];
    const refusal = new Error("threads-unavailable");
    const ok = await sendThreadCommands(async command => {
      asked.push(command);
      if (command.kind === "pin") throw refusal;
      return undefined;
    }, [{ kind: "unsnooze", key: "codex#a" }, { kind: "pin", key: "codex#a" },
      { kind: "reorder", key: "codex#a", section: "pinned" }], error => { errors.push(error); });
    expect(ok).toBe(false);
    expect(asked.map(command => command.kind)).toEqual(["unsnooze", "pin"]);
    expect(errors).toEqual([refusal]);
  });

  it("resolves true when every command is taken", async () => {
    expect(await sendThreadCommands(() => Promise.resolve(), [{ kind: "settle", key: "codex#a" }], () => {
      throw new Error("no refusal expected");
    })).toBe(true);
    expect(await sendThreadCommands(() => Promise.reject(new Error("unused")), [], () => undefined)).toBe(true);
  });
});

describe("visiting a thread whose run ends in front of the owner", () => {
  const running = view([{ key: "codex#a", lastRun: { startedAt: 1 } }, { key: "codex#b" }]);
  const ended = view([{ key: "codex#a", lastRun: { startedAt: 1, endedAt: 5, outcome: "completed" } },
    { key: "codex#b", lastRun: { startedAt: 1, endedAt: 5, outcome: "failed" } }]);
  it("visits the focused pane's thread when its run ends in a focused window", () => {
    const first = threadRunEndVisit(null, running, "codex#a", true);
    expect(first.visit).toBeNull();
    expect(threadRunEndVisit(first.seen, ended, "codex#a", true).visit).toBe("codex#a");
  });

  it("leaves it unread when the window is unfocused, another pane is focused, or the run ended before", () => {
    const first = threadRunEndVisit(null, running, "codex#a", true);
    expect(threadRunEndVisit(first.seen, ended, "codex#a", false).visit).toBeNull();
    expect(threadRunEndVisit(first.seen, ended, null, true).visit).toBeNull();
    expect(threadRunEndVisit(first.seen, ended, "codex#c", true).visit).toBeNull();
    // Opening the app on a thread that finished while it was closed is not watching it finish.
    expect(threadRunEndVisit(null, ended, "codex#a", true).visit).toBeNull();
  });
});

describe("App's Threads wiring", () => {
  it("visits a thread when it is selected, and only a Threads-surface session", () => {
    const select = app.slice(app.indexOf("const selectSession = useCallback("), app.indexOf("/* ---- Voice assistant"));
    expect(select).toContain("dispatch({ type: \"select\", key });\n    visitThread(key);");
    const visit = app.slice(app.indexOf("const visitThread = useCallback("), app.indexOf("const selectSession = useCallback("));
    expect(visit).toContain('if (summary === undefined || sessionSurfaceOf(summary) !== "chat") return;');
    expect(visit).toContain('void threadRequest({ kind: "visit", key }).catch(() => undefined);');
  });

  it("visits a thread whichever way it becomes the focused pane", () => {
    // Review, 6 Oct 2026: clicking into a pane, ⌃Tab and maximize left the
    // unread dot on a thread the owner was looking at.
    expect(threadFocusVisit("codex#a", "codex#b", true)).toBe("codex#b");
    expect(threadFocusVisit("codex#a", "codex#a", true)).toBeNull();
    expect(threadFocusVisit("codex#a", null, true)).toBeNull();
    expect(threadFocusVisit(null, "codex#b", false)).toBeNull();
    expect(app).toContain('const focusedThreadPane = surface === "chat" && centerView === "sessions"\n    ? workspace.focusedKey : null;');
    expect(app).toContain("const visit = threadFocusVisit(visitedFocus.current, focusedThreadPane, registryReady);");
    expect(app).toContain("if (visit !== null) visitThread(visit);");
    // selectSession visits on its own and says so, so the effect does not repeat it.
    const select = app.slice(app.indexOf("const selectSession = useCallback("), app.indexOf("/* ---- Voice assistant"));
    expect(select).toContain("visitThread(key);\n    // Visited here, so the focus effect below does not send it again.\n    visitedFocus.current = key;");
  });

  it("holds one fork or edit per pane, and focuses the new thread's pane", () => {
    expect(app).toContain("const [threadPaneLock] = useState(() => new ThreadPaneLock(setThreadPaneBusy));");
    expect(app).toContain("const forking = threadPaneLock.run(key, () => forkThreadAt(threadPanePort, key, keepTurns));");
    expect(app).toContain("return threadPaneLock.run(key, () => editThreadFrom(threadPanePort, thread, ordinal,");
    expect(app).toContain("conversationThread(tileThread, threadPaneBusy.has(key))");
    expect(app).toContain("pendingTileFocus.current = key;\n      selectSession(key);");
    // Turn 1's edit: settled thread's session closed unless it is busy; the
    // recovery point is a status line.
    expect(app).toContain("if (sessionBusy(sessionByKeyRef.current.get(key))) return;\n      closeLiveSession(key)");
    expect(app).toContain("notify: message => { setTurnRestoreNotice({ message, error: false }); },");
  });

  it("starts the extra models without the launch lock and reports each that did not start", () => {
    const launch = app.slice(app.indexOf("const launch = useCallback("), app.indexOf("/* ---- turn-bound checkpoints"));
    expect(launch).toContain("const extra = launchOptions.extra === true;");
    expect(launch).toMatch(/if \(!extra\) \{\s*launchingRef\.current = true;\s*setLaunching\(true\);/u);
    expect(launch).toContain("if (extra) throw err;");
    expect(app).toContain("report: (message: string): void => { setActionError(message); },");
  });

  it("leaves focus where an Archive put it: a closing pane takes focus only from itself", () => {
    // Hands-free run, 6 Oct 2026: archiving a thread no pane showed moved
    // focus from the sidebar's next row to the focused pane's title.
    const close = app.slice(app.indexOf("const closeWorkspaceTile = useCallback("),
      app.indexOf("// A closed pane's stored page is dead weight"));
    expect(close).toContain("|| workspaceTile(key)?.contains(active) === true;");
    expect(close).toContain("if (focusLeaving && current.openKeys.includes(key)) pendingTileFocus.current = next.focusedKey;");
  });

  it("lets an action error fade without taking focus from where the owner is", () => {
    const dismiss = app.slice(app.indexOf("const dismissActionError = useCallback("),
      app.indexOf("/* ---- permission mode, mid-session"));
    expect(dismiss).toContain('|| active.closest(".corner-notice, .sidebar-say-error") !== null;');
    expect(dismiss).toContain("if (fromError) focusAfterNoticeDismissal();");
  });

  it("offers archive's Undo only once the archive happened", () => {
    const hide = app.slice(app.indexOf("const hideSessionLocally = useCallback("), app.indexOf("const addContextFor = useCallback("));
    expect(hide).toContain("(key: SessionKey): Promise<boolean> =>");
    expect(hide).toMatch(/return true;\s*\}\)\s*\.catch\(\(err: unknown\) => \{\s*failAction\(err\);\s*return false;/u);
  });

  it("shows a diff panel hidden behind a subagent detail, and returns focus to the toggle on close", () => {
    expect(app).toContain("const action = threadDiffToggleAction(current?.key ?? null, key, subagentDetailRef.current !== null);");
    expect(app).toContain("onClose={closeThreadDiffPanel}");
    expect(app).toContain("<ThreadDiffToggle open={threadDiff.target?.key === key && subagentDetail === null}");
  });

  it("hands the Sidebar the inbox on the Threads surface only, with no second subscription", () => {
    expect(app).toContain('threadInbox={surface === "chat" ? threadInbox : undefined}');
    expect(app.match(/useThreads\(window\.iface\)/gu)).toHaveLength(1);
    expect(app).not.toContain("void threads;");
    expect(app).toContain("watchingKey: threadsUncovered && centerView === \"sessions\" ? workspace.focusedKey : null,");
    expect(app).toContain("shortcuts: threadsUncovered, onError: failAction, onUnarchive: unhideSession,");
    expect(app).toContain("void window.iface.archive(key, false).then(() => refreshAll()).catch(failAction);");
  });

  it("keeps the scope a renderer preference and reads the window's focus at the run end", () => {
    expect(THREAD_SCOPE_KEY).toBe("interface.threads.scope");
    expect(hook).toContain("threadRunEndVisit(runEnds.current, view, watchingKey, document.hasFocus())");
  });

  it("opens the New thread card on the scoped project, and a pane's split + on that pane's project", () => {
    expect(app).toContain("scopeProject: threadScopeRoot(threadInbox.model.scope),");
    const split = app.slice(app.indexOf("const splitThreadTile = useCallback("),
      app.indexOf("/** A Threads pane's head: its branch chip and lineage, and its Diff toggle. */"));
    // The thread's own project (a worktree thread's is not its worktree), seeded
    // only once the sheet is opening.
    expect(split).toContain("const project = threadProjectRoot(summary, thread);");
    expect(split).toContain("if (splitWorkspaceTile(project)) threadCardSeed(project);");
    expect(app).toContain("onSplit={() => { splitThreadTile(summary); }}");
    expect(app).toContain('if (surfaceRef.current === "chat" && !launchingRef.current) threadCardSeed(projectPath);');
  });

  it("lists every thread Rust records, however short its transcript", () => {
    expect(app).toContain("sidebarSessions, selectedKey, threadListingRevealed(revealed, sidebarSessions, threads.view),");
  });

  it("reads Settings → Threads from Rust's view and writes the whole object with setSettings", () => {
    expect(app).toContain("threadSettings={threads.view?.settings ?? null}");
    expect(app).toContain("onThreadSettingsChange={changeThreadSettings}");
    expect(app).toContain('threadRequest({ kind: "setSettings", settings })');
  });

  it("names the surface Threads and keeps its internal chat id", () => {
    expect(app).toContain('chat: "Threads",');
    expect(app).toContain('chat: "New thread",');
    expect(app).toContain('chat: "No threads yet.",');
    expect(app).toContain('chat: { title: "Start a thread", description: "Ask a question, or pick a project and give an agent work." },');
    const goToThreads = shellPaletteCommands({
      newLabel: "New thread", newDescription: "", attentionItems: 0, terminalsShown: false,
      openPanes: 0, focusedPane: false, maximized: false, sidebarOpen: true, launching: false,
      refreshing: false,
    }).find(command => command.id === "surface.chat");
    expect(goToThreads).toMatchObject({ label: "Go to Threads", keywords: ["threads", "chat", "conversation"] });
  });
});
