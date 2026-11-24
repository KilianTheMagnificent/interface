import { describe, expect, it } from "vitest";
import type { SessionSummary } from "../shared/engine";
import type { ThreadCommand, ThreadControlResponse, ThreadsView } from "../shared/threads";
import fixture from "../fixtures/threads-v34.json";
import {
  COMPOSER_SCOPE_DEFAULTS, COMPOSER_TEAM_DEFAULTS, type ComposerLaunchForm, type EngineChoices,
} from "./composer-model";
import {
  checkedThreadWorkspace, EMPTY_THREAD_CARD_MEMORY, initialThreadProject, launchThreadCard, parseThreadCardMemory,
  recentThreadProject,
  rememberedThreadWorkspace, rememberThreadProject, rememberThreadWorkspace,
  serializeThreadCardMemory, threadCardAllowsSeveralModels, threadCardEngines, threadCardKind,
  threadCardOpening, threadCardProjects, threadCardWording, threadExtrasFailureText,
  threadFanOutForms, threadFanOutName, threadWorkspaceLine,
  type ThreadLaunchOutcome, type ThreadLaunchPort,
} from "./thread-card";

/* ==========================================================================
   THE NEW THREAD CARD. "No project" is today's chat card exactly; a project
   is a coding session with every engine and a workspace Rust prepares. These
   pin the engine lists per mode, what the card remembers per project, and the
   prepare → launch → bind sequence (discard when the launch fails).
   ========================================================================== */

const view = fixture.view as unknown as ThreadsView;
const choices = (over: Partial<EngineChoices> = {}): EngineChoices =>
  ({ models: [], efforts: [], permissionModes: [], ...over });
const catalogue: Record<string, EngineChoices> = {
  claude: choices({ chat: true }), codex: choices(), "acp:grok": choices(),
};
const choicesFor = (engine: string): EngineChoices => catalogue[engine] ?? choices();
const engines = [{ id: "claude" }, { id: "codex" }, { id: "acp:grok" }];

const form = (over: Partial<ComposerLaunchForm> = {}): ComposerLaunchForm => ({
  engine: "claude", cwd: "/Users/owner/Developer/Projects/atlas", prompt: "Fix the flaky login test",
  model: "sonnet", effort: "", permissionMode: "auto", kind: "code",
  ...COMPOSER_SCOPE_DEFAULTS, ...COMPOSER_TEAM_DEFAULTS, ...over,
});

describe("engines per mode", () => {
  it("offers the chat engines only with No project, every engine with a project", () => {
    expect(threadCardKind(null)).toBe("chat");
    expect(threadCardKind("/p")).toBe("code");
    expect(threadCardEngines(null, engines, choicesFor).map(engine => engine.id)).toEqual(["claude"]);
    expect(threadCardEngines("/p", engines, choicesFor).map(engine => engine.id))
      .toEqual(["claude", "codex", "acp:grok"]);
  });

  it("allows several models only for a project with a new worktree each", () => {
    expect(threadCardAllowsSeveralModels("/p", { mode: "worktree" })).toBe(true);
    expect(threadCardAllowsSeveralModels("/p", { mode: "local" })).toBe(false);
    expect(threadCardAllowsSeveralModels(null, { mode: "worktree" })).toBe(false);
  });
});

describe("the card's words", () => {
  it("asks what to build in a project, and an open question with No project", () => {
    expect(threadCardWording("/p")).toEqual({
      lead: "What should we build in", trail: "?",
      placeholder: "Describe the task. @ mentions a file, / runs a command.",
    });
    // The picker follows a whole sentence here, so nothing trails it.
    expect(threadCardWording(null)).toEqual({
      lead: "What’s on your mind?", trail: "", placeholder: "Ask anything.",
    });
  });

  it("says where each workspace's edits land", () => {
    expect(threadWorkspaceLine({ mode: "local" }, "atlas")).toBe("Edits land in atlas itself.");
    expect(threadWorkspaceLine({ mode: "worktree", baseRef: "main" }, "atlas"))
      .toBe("A new branch in its own folder.");
    expect(threadWorkspaceLine({ mode: "existing", worktreePath: "/p-hotfix" }, "atlas"))
      .toBe("Edits land in a worktree that already exists.");
  });
});

describe("recent projects", () => {
  const session = (over: Partial<SessionSummary>): SessionSummary => ({
    engine: "claude", sessionId: "s", title: "t", cwd: "", status: "idle", cloud: false,
    lastActivity: "2026-10-05T10:00:00Z", firstSeen: "2026-10-05T10:00:00Z", archived: false, ...over,
  });

  it("lists pins, then thread and session projects newest first, never a thread worktree", () => {
    const projects = threadCardProjects({
      view,
      sessions: [
        session({ projectRoot: "/Users/owner/Developer/Projects/zephyr",
          lastActivity: "2026-10-09T10:00:00Z" }),
        session({ projectRoot: "/Users/owner/Developer/Projects/.interface-worktrees/x-1a2b3c" }),
        session({ projectRoot: "/Users/owner/old/beacon", lastActivity: "2020-01-01T00:00:00Z" }),
      ],
      pinned: ["/Users/owner/Developer/Projects/pinned"],
      current: null,
    });
    expect(projects.map(project => project.root)).toEqual([
      "/Users/owner/Developer/Projects/pinned",
      "/Users/owner/Developer/Projects/zephyr",
      "/Users/owner/Developer/Projects/atlas",
      "/Users/owner/old/beacon",
    ]);
    expect(projects[2].label).toBe("atlas");
  });

  it("names two folders that share a name by their parents, and keeps the current one", () => {
    const projects = threadCardProjects({ view: null, sessions: [], pinned: ["/a/web", "/b/web"],
      current: "/c/api" });
    expect(projects.map(project => project.label)).toEqual(["web (a)", "web (b)", "api"]);
  });
});

describe("workspace mode memory", () => {
  it("round-trips, and survives corrupt or foreign data as empty", () => {
    const memory = rememberThreadProject(rememberThreadWorkspace(EMPTY_THREAD_CARD_MEMORY, "/p",
      { mode: "worktree", baseRef: "main" }), "/p");
    expect(parseThreadCardMemory(serializeThreadCardMemory(memory))).toEqual(memory);
    for (const raw of [null, "", "{", "[]", '{"version":2}', '{"version":1,"workspaces":{"/p":{"mode":"bogus"}}}']) {
      expect(parseThreadCardMemory(raw).workspaces).toEqual({});
    }
  });

  it("opens a project on its last workspace mode, Local when it has none", () => {
    let memory = rememberThreadWorkspace(EMPTY_THREAD_CARD_MEMORY, "/p", { mode: "worktree" });
    memory = rememberThreadWorkspace(memory, "/q", { mode: "local" });
    expect(rememberedThreadWorkspace(memory, "/p")).toEqual({ mode: "worktree" });
    expect(rememberedThreadWorkspace(memory, "/q")).toEqual({ mode: "local" });
    expect(rememberedThreadWorkspace(memory, "/r")).toEqual({ mode: "local" });
  });

  it("never remembers an existing worktree as a project's default workspace", () => {
    // Review, 6 Oct 2026: a card seeded on an edited thread's worktree (Edit
    // from the first message) made every later New thread in that project
    // open in it.
    let memory = rememberThreadWorkspace(EMPTY_THREAD_CARD_MEMORY, "/p", { mode: "worktree" });
    memory = rememberThreadWorkspace(memory, "/p", { mode: "existing", worktreePath: "/p-wt" });
    memory = rememberThreadWorkspace(memory, "/q", { mode: "existing", worktreePath: "/q-wt" });
    expect(rememberedThreadWorkspace(memory, "/p")).toEqual({ mode: "worktree" });
    expect(rememberedThreadWorkspace(memory, "/q")).toEqual({ mode: "local" });
    expect(Object.keys(memory.workspaces)).toEqual(["/p"]);
    // One an earlier build stored reads as nothing remembered.
    const stored = '{"version":1,"workspaces":{"/q":{"mode":"existing","worktreePath":"/q-wt"}}}';
    expect(parseThreadCardMemory(stored).workspaces).toEqual({});
  });

  it("falls back when the remembered worktree or base branch is gone", () => {
    // A worktree that is gone becomes a new one, never the project's own checkout.
    expect(checkedThreadWorkspace({ mode: "existing", worktreePath: "/q-hotfix" },
      { worktrees: [{ path: "/q-other" }] })).toEqual({ mode: "worktree" });
    const memory = rememberThreadWorkspace(EMPTY_THREAD_CARD_MEMORY, "/p", { mode: "worktree", baseRef: "gone" });
    expect(rememberedThreadWorkspace(memory, "/p", { branches: { branches: ["main"] } }))
      .toEqual({ mode: "worktree" });
  });

  it("offers no worktree in a folder that is not a git repository", () => {
    const notGit = { unavailable: "Not a git repository." };
    expect(checkedThreadWorkspace({ mode: "worktree" }, notGit)).toEqual({ mode: "local" });
    expect(checkedThreadWorkspace({ mode: "existing", worktreePath: "/x" }, notGit))
      .toEqual({ mode: "local" });
    expect(checkedThreadWorkspace({ mode: "worktree" }, {})).toEqual({ mode: "worktree" });
  });

  it("remembers No project as a choice of its own, and the last project otherwise", () => {
    expect(initialThreadProject(EMPTY_THREAD_CARD_MEMORY)).toBeNull();
    expect(initialThreadProject(rememberThreadProject(EMPTY_THREAD_CARD_MEMORY, "/p"))).toBe("/p");
    expect(initialThreadProject(rememberThreadProject(EMPTY_THREAD_CARD_MEMORY, "/p"), null)).toBeNull();
  });

  it("opens on a seed, else the inbox scope's project, else the last project used", () => {
    let memory = rememberThreadProject(EMPTY_THREAD_CARD_MEMORY, "/last");
    memory = rememberThreadWorkspace(memory, "/scoped", { mode: "worktree", baseRef: "main" });
    // All projects says nothing: the last project, in its remembered workspace.
    expect(threadCardOpening(memory, null, undefined)).toEqual({ project: "/last", workspace: { mode: "local" } });
    // A scoped inbox opens the card on that project, in the workspace used there last.
    expect(threadCardOpening(memory, null, "/scoped"))
      .toEqual({ project: "/scoped", workspace: { mode: "worktree", baseRef: "main" } });
    // Scoped to No project: a plain chat.
    expect(threadCardOpening(memory, null, null)).toEqual({ project: null, workspace: { mode: "local" } });
    // A pane's split + names its project and takes that project's workspace…
    expect(threadCardOpening(memory, { project: "/scoped" }, "/other"))
      .toEqual({ project: "/scoped", workspace: { mode: "worktree", baseRef: "main" } });
    // …a plain chat pane's is No project, whatever the scope says…
    expect(threadCardOpening(memory, { project: null }, "/scoped")).toEqual({ project: null, workspace: { mode: "local" } });
    // …and Edit from turn 1 brings its own workspace, and its note.
    expect(threadCardOpening(memory, { project: "/scoped", workspace: { mode: "existing", worktreePath: "/wt" } }, undefined))
      .toEqual({ project: "/scoped", workspace: { mode: "existing", worktreePath: "/wt" } });
    expect(threadCardOpening(memory, { project: "/scoped", note: "Files restored" }, undefined).note)
      .toBe("Files restored");
    // What Rust listed still checks the remembered workspace.
    expect(threadCardOpening(memory, null, "/scoped", () => ({ unavailable: "Not a git repository." })))
      .toEqual({ project: "/scoped", workspace: { mode: "local" } });
  });

  it("keeps the newest projects when the map is full", () => {
    let memory = EMPTY_THREAD_CARD_MEMORY;
    for (let index = 0; index < 70; index += 1) {
      memory = rememberThreadWorkspace(memory, `/p${index}`, { mode: "local" });
    }
    expect(Object.keys(memory.workspaces)).toHaveLength(64);
    expect(Object.keys(memory.workspaces)[0]).toBe("/p69");
    expect(memory.workspaces["/p0"]).toBeUndefined();
  });
});

describe("launchThreadCard (prepare → launch → bind)", () => {
  const prepared = fixture.response.prepared;
  function fakePort(options: {
    prepare?: "fail"; launch?: "fail" | "throw";
    /** Per model: an extra whose prepare or launch fails, or whose launch waits. */
    models?: Record<string, "prepare" | "launch" | "wait">;
  } = {}) {
    const calls: ThreadCommand[] = [];
    const launches: { form: ComposerLaunchForm; background: boolean; extra: boolean }[] = [];
    const warnings: string[] = [];
    const waiting: (() => void)[] = [];
    let next = 0;
    const port: ThreadLaunchPort = {
      async request(command) {
        calls.push(command);
        if (command.kind === "prepareWorkspace" && (options.prepare === "fail"
            || Object.entries(options.models ?? {}).some(([model, failure]) =>
              failure === "prepare" && command.hint === `prompt for ${model}`))) {
          throw new Error("Not a git repository: /Users/owner/Downloads");
        }
        const answer = { view } as ThreadControlResponse;
        if (command.kind === "prepareWorkspace") {
          return { ...answer, prepared: { ...prepared, token: `tok-${calls.length}`,
            cwd: `/wt/${calls.length}` } } as ThreadControlResponse;
        }
        if (command.kind === "bind" && command.key === "claude#bind-fails") throw new Error("store busy");
        return answer;
      },
      async launch(launchForm, _scope, _context, { background, extra }) {
        launches.push({ form: launchForm, background, extra: extra === true });
        if (options.launch === "fail") return null;
        if (options.launch === "throw") throw new Error("boom");
        const failure = options.models?.[launchForm.model];
        if (failure === "launch") throw new Error("The provider is not signed in.");
        if (failure === "wait") await new Promise<void>(resolve => { waiting.push(resolve); });
        next += 1;
        return { key: `${launchForm.engine}#s${next}`, prompt: launchForm.prompt };
      },
      warn: message => { warnings.push(message); },
    };
    return { port, calls, launches, warnings, waiting };
  }
  const plan = (over: Partial<Parameters<typeof launchThreadCard>[1]> = {}) => ({
    project: "/Users/owner/Developer/Projects/atlas", workspace: { mode: "local" as const },
    forms: [form()], scope: {}, context: [], background: false, ...over,
  });
  const started = (outcome: ThreadLaunchOutcome) => {
    expect(outcome.status).toBe("launched");
    if (outcome.status !== "launched") throw new Error("not launched");
    return outcome;
  };

  it("prepares the workspace, launches a code session in its folder, then binds with the token", async () => {
    const { port, calls, launches } = fakePort();
    const outcome = started(await launchThreadCard(port, plan({
      workspace: { mode: "worktree", baseRef: "main" } })));
    expect(outcome.key).toBe("claude#s1");
    await expect(outcome.extras).resolves.toEqual({ keys: [], failures: [] });
    expect(calls[0]).toEqual({ kind: "prepareWorkspace", projectRoot: "/Users/owner/Developer/Projects/atlas",
      mode: "worktree", baseRef: "main", hint: "Fix the flaky login test" });
    expect(launches[0].form).toMatchObject({ kind: "code", cwd: "/wt/1" });
    expect(launches[0].extra).toBe(false);
    expect(calls[1]).toEqual({ kind: "bind", key: "claude#s1", token: "tok-1",
      prompt: "Fix the flaky login test", titleSource: "prompt" });
  });

  it("sends the existing worktree's path for that mode", async () => {
    const { port, calls } = fakePort();
    await launchThreadCard(port, plan({ workspace: { mode: "existing", worktreePath: "/x" } }));
    expect(calls[0]).toMatchObject({ kind: "prepareWorkspace", mode: "existing", worktreePath: "/x" });
  });

  it("discards the prepared workspace when the launch fails, and binds nothing", async () => {
    for (const failure of ["fail", "throw"] as const) {
      const { port, calls } = fakePort({ launch: failure });
      expect(await launchThreadCard(port, plan())).toEqual({ status: "launch-failed" });
      expect(calls.map(call => call.kind)).toEqual(["prepareWorkspace", "discardWorkspace"]);
      expect(calls[1]).toEqual({ kind: "discardWorkspace", token: "tok-1" });
    }
  });

  it("starts nothing and hands back Rust's message when prepare is refused", async () => {
    const { port, calls, launches } = fakePort({ prepare: "fail" });
    const outcome = await launchThreadCard(port, plan());
    expect(outcome).toEqual({ status: "prepare-failed",
      message: "Not a git repository: /Users/owner/Downloads" });
    expect(launches).toHaveLength(0);
    expect(calls.map(call => call.kind)).toEqual(["prepareWorkspace"]);
  });

  it("launches plain chat as kind chat and binds without a token", async () => {
    const { port, calls, launches } = fakePort();
    await launchThreadCard(port, plan({ project: null, forms: [form({ kind: "chat", cwd: "" })] }));
    expect(launches[0].form.kind).toBe("chat");
    expect(calls).toEqual([{ kind: "bind", key: "claude#s1", prompt: "Fix the flaky login test" }]);
  });

  it("retains launch attachments until every background model finishes and releases failed preparation", async () => {
    const image = { id: "image", kind: "file" as const, label: "Sketch", value: "/images/sketch.png" };
    const { port, waiting } = fakePort({ models: { slow: "wait" } });
    let retained = false;
    port.captureContext = items => { expect(items).toEqual([image]); retained = true; return () => { retained = false; }; };
    const launching = launchThreadCard(port, plan({ context: [image], workspace: { mode: "worktree" }, forms: [form(), form({ model: "slow" })] }));
    expect(retained).toBe(true);
    const outcome = started(await launching);
    for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
    expect(waiting).toHaveLength(1);
    expect(retained).toBe(true);
    waiting[0](); await outcome.extras;
    expect(retained).toBe(false);
    const failed = fakePort({ prepare: "fail" }).port;
    failed.captureContext = port.captureContext;
    await launchThreadCard(failed, plan({ context: [image] }));
    expect(retained).toBe(false);
  });

  it("starts one thread per model, each in its own new worktree, the extras in the background", async () => {
    const { port, calls, launches } = fakePort();
    const outcome = started(await launchThreadCard(port, plan({ workspace: { mode: "worktree" },
      forms: [form(), form({ engine: "codex", model: "gpt-6" })] })));
    expect(outcome.key).toBe("claude#s1");
    await expect(outcome.extras).resolves.toEqual({ keys: ["codex#s2"], failures: [] });
    expect(calls.filter(call => call.kind === "prepareWorkspace")).toHaveLength(2);
    expect(launches.map(entry => [entry.form.cwd, entry.background, entry.extra]))
      .toEqual([["/wt/1", false, false], ["/wt/3", true, true]]);
  });

  it("names a multi-model fan-out's threads by prompt and model, so their rows differ", async () => {
    // Review, 6 Oct 2026: every thread of a fan-out had the same title.
    const prompt = "Fix the flaky login test in the checkout flow,\n  then add a regression test for it please";
    expect(threadFanOutName(prompt, "Opus 5"))
      .toBe("Fix the flaky login test in the checkout flow, then add a re · Opus 5");
    expect(threadFanOutName("Short one", "GPT-6")).toBe("Short one · GPT-6");
    const label = (each: ComposerLaunchForm) => each.model.toUpperCase();
    const named = threadFanOutForms([form({ prompt }), form({ engine: "codex", model: "gpt-6", prompt })], label);
    expect(named.map(each => each.name)).toEqual([threadFanOutName(prompt, "SONNET"),
      threadFanOutName(prompt, "GPT-6")]);
    // One model is not a fan-out: its prompt stays its title.
    expect(threadFanOutForms([form()], label)[0].name).toBeUndefined();
    // The names reach the launches.
    const { port, launches } = fakePort();
    const outcome = started(await launchThreadCard(port, plan({ workspace: { mode: "worktree" }, forms: named })));
    await outcome.extras;
    expect(launches.map(entry => entry.form.name)).toEqual(named.map(each => each.name));
  });

  it("answers once the first thread started, and reports each extra model that did not", async () => {
    // Review, 6 Oct 2026: an extra model's failure was silent, and the card
    // held the launch lock (freezing navigation) until every model started.
    const { port, waiting } = fakePort({ models: { slow: "wait", gone: "prepare", out: "launch" } });
    const outcome = started(await launchThreadCard(port, plan({ workspace: { mode: "worktree" },
      forms: [form(), form({ model: "slow", prompt: "prompt for slow" }),
        form({ model: "gone", prompt: "prompt for gone" }), form({ model: "out", prompt: "prompt for out" })] })));
    // The first thread is answered while the second model is still starting.
    expect(outcome.key).toBe("claude#s1");
    let extrasDone = false;
    void outcome.extras.then(() => { extrasDone = true; });
    await Promise.resolve();
    expect(extrasDone).toBe(false);
    await new Promise(resolve => { setTimeout(resolve, 0); });
    waiting.forEach(resolve => { resolve(); });
    const extras = await outcome.extras;
    expect(extras.keys).toEqual(["claude#s2"]);
    expect(extras.failures).toEqual([
      { model: "gone", message: "Not a git repository: /Users/owner/Downloads" },
      { model: "out", message: "The provider is not signed in." },
    ]);
    expect(threadExtrasFailureText(extras.failures)).toBe("2 threads did not start. "
      + "gone: Not a git repository: /Users/owner/Downloads · out: The provider is not signed in.");
    expect(threadExtrasFailureText(extras.failures.slice(1)))
      .toBe("The out thread did not start: The provider is not signed in.");
    expect(threadExtrasFailureText([])).toBeNull();
  });

  it("starts no extra model when the first one fails", async () => {
    const { port, launches } = fakePort({ prepare: "fail" });
    await launchThreadCard(port, plan({ workspace: { mode: "worktree" },
      forms: [form(), form({ engine: "codex", model: "gpt-6" })] }));
    expect(launches).toHaveLength(0);
  });

  it("ignores extra models outside a new worktree", async () => {
    const { port, launches } = fakePort();
    const outcome = started(await launchThreadCard(port, plan({ forms: [form(), form({ engine: "codex" })] })));
    await outcome.extras;
    expect(launches).toHaveLength(1);
  });

  it("starts in the background on ⌘Enter", async () => {
    const { port, launches } = fakePort();
    await launchThreadCard(port, plan({ background: true }));
    expect(launches[0].background).toBe(true);
  });

  it("reports a failed bind without undoing the running session", async () => {
    const { port, warnings } = fakePort();
    const bindFails: ThreadLaunchPort = { ...port, launch: async () => ({ key: "claude#bind-fails", prompt: "p" }) };
    expect(started(await launchThreadCard(bindFails, plan())).key).toBe("claude#bind-fails");
    expect(warnings[0]).toContain("store busy");
  });
});

/* Review A5 (2026-10-14): a card that has never launched opened on "No
   project" even with threads in a project right beside it. */
describe("a first card's project", () => {
  const thread = (root: string | null, createdAt: number, lastVisitedAt?: number) => ({
    createdAt, ...(lastVisitedAt === undefined ? {} : { lastVisitedAt }),
    workspace: root === null ? { mode: "none" as const, projectRoot: "" }
      : { mode: "local" as const, projectRoot: root },
  });

  it("opens on the project of the thread used last when nothing was launched from it", () => {
    const threads = [thread("/a/web", 10), thread("/a/api", 5, 40), thread(null, 50)];
    expect(recentThreadProject(threads as never)).toBe("/a/api");
    expect(initialThreadProject(EMPTY_THREAD_CARD_MEMORY, undefined, "/a/api")).toBe("/a/api");
  });

  it("keeps what the card itself remembers, No project included, and the inbox scope", () => {
    expect(initialThreadProject({ workspaces: {}, lastProject: null }, undefined, "/a/api")).toBeNull();
    expect(initialThreadProject({ workspaces: {}, lastProject: "/a/web" }, undefined, "/a/api")).toBe("/a/web");
    expect(initialThreadProject(EMPTY_THREAD_CARD_MEMORY, "/a/scope", "/a/api")).toBe("/a/scope");
    expect(recentThreadProject([])).toBeUndefined();
  });
});
