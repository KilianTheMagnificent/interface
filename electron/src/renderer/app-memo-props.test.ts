import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// App re-renders on every event batch, registry push, voice frame and
// composer keystroke. These surfaces are memo()'d, and an inline arrow,
// object or array prop defeats that on every one of those renders.
const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const sessionTile = readFileSync(new URL("./components/SessionTile.tsx", import.meta.url), "utf8");
const outputPreviewPanel = readFileSync(new URL("./components/OutputPreviewPanel.tsx", import.meta.url), "utf8");
const useThreads = readFileSync(new URL("./use-threads.ts", import.meta.url), "utf8");
const useThreadBridges = readFileSync(new URL("./use-thread-bridges.ts", import.meta.url), "utf8");

function elementProps(name: string): string {
  const start = app.search(new RegExp(`<${name}\\s`, "u"));
  expect(start, `<${name}> element not found in App.tsx`).toBeGreaterThanOrEqual(0);
  return app.slice(start, app.indexOf("/>", start));
}

describe("App props into memoized surfaces", () => {
  it.each(["IntegratedTerminalCenter", "Sidebar", "AgentCanvas"])("passes <%s> no inline callbacks or literals", name => {
    expect(app).toMatch(new RegExp(`const ${name} = memo\\(`, "u"));
    const props = elementProps(name);
    expect(props).not.toContain("=>");
    expect(props).not.toMatch(/=\{\s*[{[]/u);
  });

  it("passes <OutputPreviewPanel> no inline callbacks or literals", () => {
    // Declared in its own file; it stays mounted (hidden) after the first preview.
    expect(outputPreviewPanel).toContain("export const OutputPreviewPanel = memo(function OutputPreviewPanel(");
    const props = elementProps("OutputPreviewPanel");
    expect(props).not.toContain("=>");
    expect(props).not.toMatch(/=\{\s*[{[]/u);
  });

  it("keeps the thread callbacks stable: no dependency array names the whole `threads` handle", () => {
    // `useThreads` answers a new `{ view, request }` whenever the view changes;
    // a callback that lists it re-creates threadBridges (Sidebar) and
    // onContinueAsThread (IntegratedTerminalCenter). Use `threadRequest`.
    expect(useThreads).toContain("return useMemo(() => ({ view, request }), [view, request]);");
    const deps = [...`${app}\n${useThreadBridges}`.matchAll(/[)}],\s*\[([^\][]*)\]\s*\)/gu)].map(match => match[1] ?? "");
    expect(deps.length).toBeGreaterThan(100);
    expect(deps.filter(list => /(?<![.\w$])threads(?![\w$.])/u.test(list))).toEqual([]);
  });

  it("hands each <SessionTile> values and one committed actions object", () => {
    expect(sessionTile).toContain("export const SessionTile = memo(function SessionTile(");
    // The tile reads no App state and calls no host itself: App stays the one
    // `window.iface` caller and hands the pane's two host reads over as actions.
    expect(sessionTile).not.toMatch(/window\.iface\./u);
    expect(app).toContain("loadProviderCommands: scope => window.iface.listProviderCommands(scope),");
    expect(app).toContain("pathForFile: file => window.iface.pathForFile(file),");
    expect(app).toContain("const sessionTileActions = useCommittedActions<SessionTileActions>({");
    const props = elementProps("SessionTile");
    expect(props).toContain("actions={sessionTileActions}");
    // Derived values may use arrows inside them; no prop may BE a callback or
    // a fresh literal. Pane callbacks bind the key inside the tile.
    expect(props).not.toMatch(/=\{\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>/u);
    expect(props).not.toMatch(/=\{\s*[{[]/u);
  });
});

describe("the pane's props into its memoized composer", () => {
  it("hands <ConversationComposer> no caller callback as it came: each goes through the pane's forwarders", () => {
    // SessionTile binds the thread key in a fresh arrow on every draw, and the
    // pane draws on every streamed event; a raw `props.onX` here redraws the
    // memo()'d composer with it (T3 section 12's /btw did, 8 of 15 draws in one turn).
    const conversation = readFileSync(new URL("./components/Conversation.tsx", import.meta.url), "utf8");
    const start = conversation.indexOf("<ConversationComposer");
    expect(start).toBeGreaterThanOrEqual(0);
    const props = conversation.slice(start, conversation.indexOf("/>", start));
    expect(props.match(/\bon[A-Z]\w*=\{props\.on[A-Z]\w*\}/gu) ?? []).toEqual([]);
  });
});

describe("engine choices", () => {
  it("answers each engine once per catalogue revision, not once per streamed frame", () => {
    expect(app).toMatch(/const choicesCache = useMemo\(\(\) => new Map<string, EngineChoices>\(\),\s*\[state\.engineHealth, catalogueRevision\]\);/u);
    const body = app.slice(app.indexOf("const choicesFor = useCallback("), app.indexOf("}, [state.engineHealth, choicesCache]);"));
    expect(body).toMatch(/^const choicesFor = useCallback\(\(engine: string\): EngineChoices => \{\s*const known = choicesCache\.get\(engine\);\s*if \(known !== undefined\) return known;/u);
    expect(body).toContain("choicesCache.set(engine, value);");
    expect(body).not.toContain("log === logs[i]");
  });
});

describe("window focus", () => {
  it("re-reads no capability map, no loading state and no hidden canvas", async () => {
    const { readFileSync: read } = await import("node:fs");
    const orchestration = read(new URL("./use-orchestration.ts", import.meta.url), "utf8");
    expect(app).toContain('kind === "focus" ? undefined : window.iface.engineOps()');
    expect(app).toContain('loadCollaborations({ quiet: kind === "focus" }),');
    expect(app).toContain("useCanvasReads(window.iface, activeCanvasRoot, orchestrationReads, canvasShown);");
    expect(orchestration.match(/if \(!enabled\) return;/gu)).toHaveLength(2);
    expect(orchestration).toContain("}, [api, projectRoot, reads, enabled]);");
    expect(orchestration).toContain("}, [api, reads, enabled]);");
  });
});

describe("reference stability across registry and ship refreshes", () => {
  it("keeps outputFeedbackTargets and withShip references stable when visible fields do not change", async () => {
    const { outputFeedbackTargetRows } = await import("./app-session-projection");
    const { withShip } = await import("./thread-pane");
    const first = outputFeedbackTargetRows(undefined, [
      {
        engine: "codex",
        sessionId: "s1",
        title: "First",
        cwd: "/work",
        status: "idle",
        cloud: false,
        lastActivity: "2026-10-20T00:00:00Z",
        firstSeen: "2026-10-20T00:00:00Z",
        archived: false,
      },
    ]);
    const second = outputFeedbackTargetRows(first, [
      {
        engine: "codex",
        sessionId: "s1",
        title: "First",
        cwd: "/work",
        status: "working",
        cloud: false,
        lastActivity: "2026-10-20T00:00:05Z",
        firstSeen: "2026-10-20T00:00:00Z",
        archived: false,
      },
    ]);
    expect(second).toBe(first);
    expect(second[0]).toBe(first[0]);

    const base = { turns: [], project: true, worktree: true, revertFilesBlocked: null };
    const shipA = { text: "1 commit ready", next: "push" as const, label: "Push", busy: false };
    const shipB = { text: "1 commit ready", next: "push" as const, label: "Push", busy: false };
    expect(withShip(base, shipB)).toBe(withShip(base, shipA));
  });
});
