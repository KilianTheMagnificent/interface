import { describe, expect, it } from "vitest";
import nativeResumeFixture from "../fixtures/terminal-native-resume-v53.json";
import kimiResumeFixture from "../fixtures/terminal-kimi-resume-v55.json";
import { failedReopenTerminals, relocateTerminalWorkspaces, shellFallbackRequests } from "./terminal-workspaces";
import {
  TERMINAL_LAUNCH_PROFILE_IDS,
  type TerminalLaunchSessionV1,
  type TerminalSummary,
  type TerminalWorkspaceRecord,
} from "../shared/ipc";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE,
  attachTerminalsBalanced,
  openTerminalTab,
  savedLayoutFacts,
  serializeTerminalWorkspaceState,
  terminalIdsInWorkspaceState,
} from "./terminal-workspace";
import {
  canReopenChat,
  forgetClosedTabs,
  forgetTerminal,
  forgetWorkspace,
  initialTerminalWorkspaceRecords,
  neighboringWorkspaceId,
  newChatBinding,
  newTerminalWorkspace,
  normalizeTerminalWorkspaces,
  normalizeWorkspaceName,
  reconcileLiveTerminalWorkspaceBindings,
  reconcileSavedWorkspaceNames,
  rememberLayout,
  rememberTerminal,
  shouldPersistAuthoritativeTerminalIdentity,
  terminalSwitcherRecords,
  usableTerminalTitle,
  reopenRequests,
  reopenedChatBinding,
  paneStartDirectory,
  visibleWorkspaceTerminals,
  workspaceById,
  workspaceDeleteConfirm,
  workspaceForTerminal,
} from "./terminal-workspaces";
import {
  applyTerminalLifecycleEvent, applyTerminalRegistrySnapshot,
} from "./terminal-output-state";

const NOW = (): string => "2026-08-28T00:00:00.000Z";
const CHAT_LIVE = "3e900b74-7ad0-42ac-8a7c-e2a64b7b7e04";
const CHAT_GONE = "4f9fad93-ba46-41ab-86c3-c1d94dc46d0c";
const OPENCODE_SESSION = "ses_f83a11783ffeRtqTE7hTSgvckz";

it("accepts durable host names without reverting edits made during a save", () => {
  const initial: TerminalWorkspaceRecord = { id: "w", name: "Workspace 18", createdAt: NOW(), updatedAt: NOW(), terminals: [] };
  const saved = { ...initial, name: "Study modules" };
  const current = { ...initial, layout: "new layout" };
  expect(reconcileSavedWorkspaceNames([current], [initial], [saved])).toEqual([{ ...current, name: "Study modules" }]);
  const renamed = [{ ...current, name: "My learning plan" }];
  expect(reconcileSavedWorkspaceNames(renamed, [initial], [saved])).toBe(renamed);
  expect(reconcileSavedWorkspaceNames([], [initial], [saved])).toEqual([]);
});

it("keeps a saved pane of every launch profile the host defines", () => {
  const [record] = normalizeTerminalWorkspaces([{ id: "workspace", name: "Code", createdAt: NOW(),
    updatedAt: NOW(), terminals: TERMINAL_LAUNCH_PROFILE_IDS.map((profileId, index) => ({
      id: `${CHAT_LIVE.slice(0, -1)}${index.toString()}`, title: "Pane", cwd: "/tmp/project",
      launch: { version: 1, profileId } })) }]);
  expect(record?.terminals.map(terminal => terminal.launch?.profileId))
    .toEqual([...TERMINAL_LAUNCH_PROFILE_IDS]);
});

it("preserves id-less OpenCode restore intent for two panes in one folder", () => {
  const saved = normalizeTerminalWorkspaces([{ id: "workspace", name: "Code", createdAt: NOW(), updatedAt: NOW(),
    terminals: [CHAT_LIVE, CHAT_GONE].map(id => ({ id, title: "OpenCode", cwd: "/tmp/project",
      launch: { version: 1, profileId: "opencode" }, launchSession: { version: 1, resume: false } })) }]);
  expect(saved[0].terminals.map(terminal => terminal.launchSession)).toEqual([
    { version: 1, resume: false }, { version: 1, resume: false },
  ]);
  const requests = reopenRequests(saved[0], [], () => { throw new Error("must not mint a session"); });
  expect(requests.map(request => [request.id, request.cwd, request.launchSession])).toEqual([
    [CHAT_LIVE, "/tmp/project", { version: 1, resume: true }],
    [CHAT_GONE, "/tmp/project", { version: 1, resume: true }],
  ]);
});

function ids(): () => string {
  let index = 0;
  return () => {
    index += 1;
    return `id-${index.toString()}`;
  };
}

function summary(overrides: Partial<TerminalSummary> = {}): TerminalSummary {
  return {
    id: "t-1",
    generation: 1,
    title: "Claude",
    root: "/tmp/project",
    cwd: "/tmp/project",
    state: "running",
    size: { columns: 80, rows: 24 },
    createdAt: NOW(),
    updatedAt: NOW(),
    ...overrides,
  };
}

function workspace(overrides: Partial<TerminalWorkspaceRecord> = {}): TerminalWorkspaceRecord {
  return {
    ...newTerminalWorkspace("Backend", ids(), NOW),
    ...overrides,
  };
}

describe("OMP workspaces", () => {
  it("retains OMP on reload without inventing a provider conversation", () => {
    const [record] = normalizeTerminalWorkspaces([workspace({ terminals: [{
      id: CHAT_LIVE, title: "OMP", cwd: "/tmp/project", launch: { version: 1, profileId: "omp" },
    }] })]);
    expect(record.terminals[0].launch).toEqual({ version: 1, profileId: "omp" });
    expect(reopenRequests(record, [], ids())[0]).toMatchObject({
      cwd: "/tmp/project", launch: { version: 1, profileId: "omp" },
    });
    expect(reopenRequests(record, [], ids())[0].launchSession).toBeUndefined();
  });
});

describe("Muse Code workspaces", () => {
  it("retains Muse Code on reload without inventing a provider conversation", () => {
    const [record] = normalizeTerminalWorkspaces([workspace({ terminals: [{
      id: CHAT_LIVE, title: "Muse Code", cwd: "/tmp/project", launch: { version: 1, profileId: "muse" },
    }] })]);
    expect(record.terminals[0].launch).toEqual({ version: 1, profileId: "muse" });
    expect(reopenRequests(record, [], ids())[0]).toMatchObject({
      cwd: "/tmp/project", launch: { version: 1, profileId: "muse" },
    });
    expect(reopenRequests(record, [], ids())[0].launchSession).toBeUndefined();
    expect(newChatBinding("muse", ids())).toBeUndefined();
    expect(canReopenChat("muse")).toBe(true);
  });
});

describe("Kimi Code workspaces", () => {
  it("reopens Kimi Code's hook-reported session and nothing it did not report", () => {
    const [bound] = normalizeTerminalWorkspaces([workspace({ terminals: [{
      id: CHAT_LIVE, title: "Kimi Code", cwd: "/tmp/project", launch: { version: 1, profileId: "kimi" },
      launchSession: { version: 1, id: "kimi-6b0d3c8e-4f7a-4c1e-9a52-0d8e3f6a7b19", resume: true },
    }] })]);
    expect(bound.terminals[0].launch).toEqual({ version: 1, profileId: "kimi" });
    expect(reopenRequests(bound, [], ids())[0]).toMatchObject({
      cwd: "/tmp/project", launch: { version: 1, profileId: "kimi" },
      launchSession: { version: 1, id: "kimi-6b0d3c8e-4f7a-4c1e-9a52-0d8e3f6a7b19", resume: true },
    });
    // Kimi's own `session_…` spelling and a pane that never reported a
    // session both reopen fresh; Interface never mints a Kimi id.
    for (const launchSession of [
      { version: 1 as const, id: "session_6b0d3c8e-4f7a-4c1e-9a52-0d8e3f6a7b19", resume: true },
      undefined,
    ]) {
      const [record] = normalizeTerminalWorkspaces([workspace({ terminals: [{
        id: CHAT_LIVE, title: "Kimi Code", cwd: "/tmp/project", launch: { version: 1, profileId: "kimi" },
        launchSession,
      }] })]);
      expect(reopenRequests(record, [], ids())[0].launchSession).toBeUndefined();
    }
    expect(newChatBinding("kimi", ids())).toBeUndefined();
    expect(canReopenChat("kimi")).toBe(true);
  });
});

describe("OpenCode workspaces", () => {
  it("retains OpenCode on reload and reopens its proven chat", () => {
    const [plain] = normalizeTerminalWorkspaces([workspace({ terminals: [{
      id: CHAT_LIVE, title: "OpenCode", cwd: "/tmp/project", launch: { version: 1, profileId: "opencode" },
    }] })]);
    expect(plain.terminals[0].launch).toEqual({ version: 1, profileId: "opencode" });
    // A pane whose capture never converged has no chat to name; it reopens
    // fresh and the host capture re-binds whatever this pane starts.
    expect(reopenRequests(plain, [], ids())[0]).toMatchObject({
      cwd: "/tmp/project", launch: { version: 1, profileId: "opencode" },
    });
    expect(reopenRequests(plain, [], ids())[0].launchSession)
      .toEqual({ version: 1, resume: true });
    const [bound] = normalizeTerminalWorkspaces([workspace({ terminals: [{
      id: CHAT_LIVE, title: "OpenCode", cwd: "/tmp/project",
      launch: { version: 1, profileId: "opencode" },
      launchSession: { version: 1, id: OPENCODE_SESSION, resume: true },
    }] })]);
    expect(reopenRequests(bound, [], ids())[0].launchSession)
      .toEqual({ version: 1, id: OPENCODE_SESSION, resume: true });
  });
});

describe("chat bindings", () => {
  it("preserves protocol 53 and 55 native Antigravity, Muse and Kimi resume identities across reload", () => {
    for (const row of [...nativeResumeFixture, ...kimiResumeFixture]) {
      const profileId = row.profile as "gemini" | "muse" | "kimi";
      const [record] = normalizeTerminalWorkspaces([workspace({terminals:[{
        id: CHAT_LIVE, title: profileId, cwd:"/tmp/project", launch:{version:1,profileId},
        launchSession:{version:1,id:row.id,resume:true},
      }]})]);
      expect(reopenRequests(record, [], ids())[0].launchSession).toEqual({version:1,id:row.id,resume:true});
      expect(newChatBinding(profileId, ids())).toBeUndefined();
      expect(reopenedChatBinding({version:1,id:row.providerId,resume:true},profileId,ids())).toBeUndefined();
    }
  });
  it("mints an exact managed pane handle for every resumable CLI", () => {
    expect(newChatBinding("claude", ids())).toEqual({ version: 1, id: "id-1", resume: false });
    expect(newChatBinding("grok", ids())).toEqual({ version: 1, id: "id-1", resume: false });
    // Codex assigns its provider id; this is the wrapper-owned client handle.
    expect(newChatBinding("codex", ids())).toEqual({ version: 1, id: "id-1", resume: false });
    // Antigravity owns its conversation ids and rejects Gemini CLI's binding flags.
    expect(newChatBinding("gemini", ids())).toBeUndefined();
    expect(newChatBinding("omp", ids())).toBeUndefined();
    // OpenCode mints its own `ses_…` ids, so a fresh pane binds chat intent
    // only; the host capture files the provider id once OpenCode names it.
    expect(newChatBinding("opencode", ids())).toEqual({ version: 1, resume: false });
    expect(newChatBinding("qwen", ids())).toBeUndefined();
    expect(newChatBinding("shell", ids())).toBeUndefined();
    expect(newChatBinding(undefined, ids())).toBeUndefined();
  });

  it("knows which profiles can be reopened at all", () => {
    expect(canReopenChat("claude")).toBe(true);
    expect(canReopenChat("codex")).toBe(true);
    expect(canReopenChat("gemini")).toBe(true);
    expect(canReopenChat("omp")).toBe(false);
    expect(canReopenChat("opencode")).toBe(true);
    expect(canReopenChat(undefined)).toBe(false);
  });

  it("flips a binding to resume without losing the id", () => {
    expect(reopenedChatBinding({ version: 1, id: CHAT_LIVE, resume: false }))
      .toEqual({ version: 1, id: CHAT_LIVE, resume: true });
    expect(reopenedChatBinding(undefined)).toBeUndefined();
    expect(reopenedChatBinding({ version: 1, id: "malformed", resume: false }))
      .toBeUndefined();
    const futureBinding = {
      version: 2,
      id: CHAT_LIVE,
      resume: false,
    } as unknown as TerminalLaunchSessionV1;
    expect(reopenedChatBinding(futureBinding))
      .toBeUndefined();
    expect(reopenedChatBinding(null as unknown as TerminalLaunchSessionV1))
      .toBeUndefined();
    expect(reopenedChatBinding("corrupt" as unknown as TerminalLaunchSessionV1))
      .toBeUndefined();
  });

  it("sends a pre-capture Claude id through host history recovery", () => {
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: false }, "claude", () => CHAT_GONE,
    )).toEqual({ version: 1, id: CHAT_LIVE, resume: true });
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: false }, "claude",
    )).toEqual({ version: 1, id: CHAT_LIVE, resume: true });
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: true }, "claude", () => CHAT_GONE,
    )).toEqual({ version: 1, id: CHAT_LIVE, resume: true });

    // Other resumable launchers still own their existing resume grammar.
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: false }, "codex", () => CHAT_GONE,
    )).toEqual({ version: 1, id: CHAT_LIVE, resume: true });
    // A Codex thread id captured by the host is UUID v7, not a minted v4.
    const codexThread = "01a06978-1e6e-7911-bec8-2df09b45b2b6";
    expect(reopenedChatBinding(
      { version: 1, id: codexThread, resume: true }, "codex", () => CHAT_GONE,
    )).toEqual({ version: 1, id: codexThread, resume: true });
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: false }, "grok", () => CHAT_GONE,
    )).toEqual({ version: 1, id: CHAT_LIVE, resume: true });
    // OpenCode preserves exact IDs and asks for its native picker otherwise.
    expect(reopenedChatBinding(
      { version: 1, id: OPENCODE_SESSION, resume: true }, "opencode", () => CHAT_GONE,
    )).toEqual({ version: 1, id: OPENCODE_SESSION, resume: true });
    expect(reopenedChatBinding(
      { version: 1, id: OPENCODE_SESSION, resume: false }, "opencode", () => CHAT_GONE,
    )).toEqual({ version: 1, id: OPENCODE_SESSION, resume: true });
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: true }, "opencode", () => CHAT_GONE,
    )).toEqual({ version: 1, resume: true });
    expect(reopenedChatBinding(undefined, "opencode", ids()))
      .toEqual({ version: 1, resume: true });
    expect(reopenedChatBinding(
      { version: 1, id: CHAT_LIVE, resume: false }, "gemini", () => CHAT_GONE,
    )).toBeUndefined();
  });

  it("migrates a legacy resumable pane to a fresh managed handle", () => {
    expect(reopenedChatBinding(undefined, "codex", ids()))
      .toEqual({ version: 1, id: "id-1", resume: false });
    expect(reopenedChatBinding({ version: 1, resume: false }, "claude", ids()))
      .toEqual({ version: 1, resume: true });
    expect(reopenedChatBinding({ version: 1, id: "legacy-id", resume: false }, "grok", ids()))
      .toEqual({ version: 1, id: "id-1", resume: false });
    const futureBinding = {
      version: 2,
      id: CHAT_LIVE,
      resume: false,
    } as unknown as TerminalLaunchSessionV1;
    expect(reopenedChatBinding(futureBinding, "codex", ids()))
      .toEqual({ version: 1, id: "id-1", resume: false });
  });
});

describe("authoritative terminal identity persistence", () => {
  const confirmedClaude = {
    launch: { version: 1 as const, profileId: "claude" as const },
    launchSession: { version: 1 as const, id: CHAT_LIVE, resume: true },
  };

  it("persists current and legacy confirmed Claude demotions to Shell", () => {
    const shell = {
      launch: { version: 1 as const, profileId: "shell" as const },
      launchSession: undefined,
    };
    expect(shouldPersistAuthoritativeTerminalIdentity(confirmedClaude, shell)).toBe(true);
    expect(shouldPersistAuthoritativeTerminalIdentity({
      launch: undefined,
      launchSession: confirmedClaude.launchSession,
    }, shell)).toBe(true);
    expect(shouldPersistAuthoritativeTerminalIdentity(confirmedClaude, {
      launch: undefined,
      launchSession: undefined,
    })).toBe(true);
  });

  it("does not turn ordinary shell lifecycle traffic into workspace writes", () => {
    const shell = {
      launch: { version: 1 as const, profileId: "shell" as const },
      launchSession: undefined,
    };
    expect(shouldPersistAuthoritativeTerminalIdentity(undefined, shell)).toBe(false);
    expect(shouldPersistAuthoritativeTerminalIdentity(shell, shell)).toBe(false);
    expect(shouldPersistAuthoritativeTerminalIdentity({
      launch: confirmedClaude.launch,
      launchSession: { version: 1, resume: true },
    }, shell)).toBe(false);
    expect(shouldPersistAuthoritativeTerminalIdentity({
      launch: { version: 1, profileId: "grok" },
      launchSession: confirmedClaude.launchSession,
    }, shell)).toBe(false);
  });

  it("persists a confirmed Codex demotion to Shell like a Claude one", () => {
    expect(shouldPersistAuthoritativeTerminalIdentity({
      launch: { version: 1, profileId: "codex" },
      launchSession: confirmedClaude.launchSession,
    }, {
      launch: { version: 1, profileId: "shell" },
      launchSession: undefined,
    })).toBe(true);
  });

  it("preserves the existing confirmed and in-progress Claude persistence paths", () => {
    expect(shouldPersistAuthoritativeTerminalIdentity(undefined, confirmedClaude)).toBe(true);
    expect(shouldPersistAuthoritativeTerminalIdentity(undefined, {
      launch: confirmedClaude.launch,
      launchSession: { version: 1, resume: false },
    })).toBe(true);
  });
});

describe("workspace membership", () => {
  it("adopts every live terminal and the old layout on first-run migration", () => {
    const layout = '{"version":1,"panes":[]}';
    const records = initialTerminalWorkspaceRecords([], [
      summary({ id: "t-1" }),
      summary({
        id: "t-2",
        title: "Grok",
        launchSession: { version: 1, id: "chat-2", resume: false },
      }),
    ], layout, ids(), NOW);
    expect(records).toHaveLength(1);
    expect(records[0].layout).toBe(layout);
    expect(records[0].terminals.map(terminal => terminal.id)).toEqual(["t-1", "t-2"]);
    expect(records[0].terminals[1].launchSession)
      .toEqual({ version: 1, id: "chat-2", resume: false });
  });

  it("does not rewrite an existing workspace roster during startup", () => {
    const saved = workspace({ id: "saved", name: "Saved" });
    expect(initialTerminalWorkspaceRecords(
      [saved], [summary()], "replacement", ids(), NOW,
    )).toEqual([saved]);
  });

  it("reconciles a provider lifecycle binding accepted before the saved roster loads", () => {
    const terminalId = "6cc54a03-527a-4b3a-b54a-cdb829440f89";
    const saved = workspace({
      id: "saved",
      name: "Saved",
      terminals: [{
        id: terminalId,
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_GONE, resume: true },
      }],
    });
    // The lifecycle listener has already put this provider-confirmed summary
    // in the fenced registry, while the component's workspace ref is empty.
    const acceptedBeforeLoad = summary({
      id: terminalId,
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    });
    const seeded = initialTerminalWorkspaceRecords(
      [saved], [acceptedBeforeLoad], "replacement", ids(), NOW,
    );
    expect(seeded[0].terminals[0].launchSession)
      .toEqual({ version: 1, id: CHAT_GONE, resume: true });

    const reconciled = reconcileLiveTerminalWorkspaceBindings(
      seeded, [acceptedBeforeLoad], NOW,
    );
    expect(reconciled).not.toBe(seeded);
    expect(reconciled[0].terminals[0].launchSession)
      .toEqual({ version: 1, id: CHAT_LIVE, resume: true });
    // An unchanged live binding preserves identity, which keeps startup from
    // rewriting the saved roster on every launch.
    expect(reconcileLiveTerminalWorkspaceBindings(
      reconciled, [acceptedBeforeLoad], NOW,
    )).toBe(reconciled);
  });

  it("promotes a saved shell reopen identity even when its Claude binding already matches", () => {
    const terminalId = "6cc54a03-527a-4b3a-b54a-cdb829440f89";
    const binding = { version: 1 as const, id: CHAT_LIVE, resume: true };
    const provider = summary({
      id: terminalId,
      title: "Claude Code",
      launch: { version: 1, profileId: "claude" },
      launchSession: binding,
    });

    for (const launch of [
      undefined,
      { version: 1 as const, profileId: "shell" as const },
    ]) {
      const saved = workspace({
        id: `saved-${launch?.profileId ?? "legacy"}`,
        terminals: [{
          id: terminalId,
          title: "Shell",
          cwd: "/tmp/project",
          ...(launch === undefined ? {} : { launch }),
          launchSession: binding,
        }],
      });
      const reconciled = reconcileLiveTerminalWorkspaceBindings([saved], [provider], NOW);
      expect(reconciled[0].terminals[0]).toMatchObject({
        launch: { version: 1, profileId: "claude" },
        launchSession: binding,
      });
      expect(reopenRequests(reconciled[0], [], ids())).toEqual([{
        id: terminalId,
        cwd: "/tmp/project",
        title: "Claude Code",
        launch: { version: 1, profileId: "claude" },
        launchSession: binding,
      }]);
    }
  });

  it("retains a saved launch when a confirmed legacy live summary omits it", () => {
    const terminalId = "6cc54a03-527a-4b3a-b54a-cdb829440f89";
    const saved = workspace({
      id: "saved",
      terminals: [{
        id: terminalId,
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_GONE, resume: true },
      }],
    });
    const provider = summary({
      id: terminalId,
      launch: undefined,
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    });

    const reconciled = reconcileLiveTerminalWorkspaceBindings([saved], [provider], NOW);
    expect(reconciled[0].terminals[0]).toMatchObject({
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    });
    expect(reopenRequests(reconciled[0], [], ids())[0]).toMatchObject({
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    });

    const directLifecycle = rememberTerminal(
      [saved], saved.id, provider, provider.launchSession, NOW, "authoritative",
    );
    expect(directLifecycle[0].terminals[0]).toMatchObject({
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    });
  });

  it("persists a snapshot binding before its same-revision lifecycle arrives", () => {
    const terminalId = "6cc54a03-527a-4b3a-b54a-cdb829440f89";
    const saved = workspace({
      id: "saved",
      terminals: [{
        id: terminalId,
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, resume: true },
      }],
    });
    const provider = summary({
      id: terminalId,
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    });
    const currentRegistry = { revision: 4, terminals: [] as TerminalSummary[] };
    const snapshot = applyTerminalRegistrySnapshot(currentRegistry, {
      revision: 5,
      terminals: [provider],
    });
    const reconciled = reconcileLiveTerminalWorkspaceBindings(
      [saved], snapshot.terminals, NOW,
    );
    expect(reconciled[0].terminals[0].launchSession)
      .toEqual({ version: 1, id: CHAT_LIVE, resume: true });

    const lateLifecycle = applyTerminalLifecycleEvent(snapshot, {
      kind: "upsert",
      revision: 5,
      terminal: provider,
    });
    expect(lateLifecycle.registry).toBe(snapshot);
    expect(lateLifecycle.payloadApplied).toBe(false);
  });

  it("persists a snapshot demotion before its same-revision lifecycle arrives", () => {
    const terminalId = "6cc54a03-527a-4b3a-b54a-cdb829440f89";
    const saved = workspace({
      id: "saved",
      terminals: [{
        id: terminalId,
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_LIVE, resume: true },
      }],
    });
    const shell = summary({
      id: terminalId,
      title: "Shell",
      launch: { version: 1, profileId: "shell" },
      launchSession: undefined,
    });
    const currentRegistry = { revision: 4, terminals: [] as TerminalSummary[] };
    const snapshot = applyTerminalRegistrySnapshot(currentRegistry, {
      revision: 5,
      terminals: [shell],
    });

    const reconciled = reconcileLiveTerminalWorkspaceBindings(
      [saved], snapshot.terminals, NOW,
    );
    expect(reconciled[0].terminals[0]).toMatchObject({
      launch: { version: 1, profileId: "shell" },
    });
    expect(reconciled[0].terminals[0].launchSession).toBeUndefined();

    const lateLifecycle = applyTerminalLifecycleEvent(snapshot, {
      kind: "upsert",
      revision: 5,
      terminal: shell,
    });
    expect(lateLifecycle.registry).toBe(snapshot);
    expect(lateLifecycle.payloadApplied).toBe(false);
  });

  it("files and drops a terminal", () => {
    const created = workspace();
    const binding = { version: 1 as const, id: "chat-1", resume: false };
    let records = rememberTerminal([created], created.id, summary(), binding, NOW);
    expect(records[0].terminals).toEqual([{
      id: "t-1",
      title: "Claude",
      cwd: "/tmp/project",
      launchSession: binding,
    }]);
    expect(workspaceForTerminal(records, "t-1")?.id).toBe(created.id);

    // Filing the same terminal twice replaces rather than duplicates.
    records = rememberTerminal(records, created.id, summary({ title: "Claude 2" }), binding, NOW);
    expect(records[0].terminals).toHaveLength(1);
    expect(records[0].terminals[0].title).toBe("Claude 2");

    records = forgetTerminal(records, "t-1", NOW);
    expect(records[0].terminals).toEqual([]);
    expect(workspaceForTerminal(records, "t-1")).toBeUndefined();
  });

  it("never files a title the host would refuse, so one OSC rename cannot drop the roster", () => {
    expect(usableTerminalTitle("")).toBe("Terminal");
    expect(usableTerminalTitle("\u0000\u0007")).toBe("Terminal");
    expect(usableTerminalTitle(`  ${"a".repeat(200)}  `).length).toBe(120);
    expect(usableTerminalTitle(`${"😀".repeat(121)}`)).toBe("😀".repeat(120));
    const created = workspace();
    const records = rememberTerminal([created], created.id, summary({ title: "" }), undefined, NOW);
    expect(records[0].terminals[0].title).toBe("Terminal");
    expect(rememberTerminal([created], created.id, summary({ cwd: "" }), undefined, NOW)[0]
      .terminals).toEqual([]);
  });

  it("prefers the host's own binding over the caller's", () => {
    const created = workspace();
    const fromHost = { version: 1 as const, id: "host", resume: false };
    const records = rememberTerminal([created], created.id, summary({ launchSession: fromHost }),
      { version: 1, id: "caller", resume: false }, NOW);
    expect(records[0].terminals[0].launchSession).toEqual(fromHost);
  });

  it("does not let a late fresh summary erase a confirmed provider identity", () => {
    const confirmed = { version: 1 as const, id: CHAT_LIVE, resume: true };
    const created = workspace({
      terminals: [{
        id: "t-1",
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: confirmed,
      }],
    });
    const records = rememberTerminal(
      [created],
      created.id,
      summary({
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, resume: false },
      }),
      { version: 1, id: CHAT_GONE, resume: false },
      NOW,
    );
    expect(records[0].terminals[0].launchSession).toEqual(confirmed);
  });

  it("keeps a confirmed launch and binding together until host authority replaces both", () => {
    const confirmed = { version: 1 as const, id: CHAT_LIVE, resume: true };
    const created = workspace({
      terminals: [{
        id: "t-1",
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: confirmed,
      }],
    });
    const lateShell = summary({
      title: "Shell",
      launch: { version: 1, profileId: "shell" },
      launchSession: undefined,
    });

    const preserved = rememberTerminal([created], created.id, lateShell, undefined, NOW);
    expect(preserved[0].terminals[0]).toMatchObject({
      launch: { version: 1, profileId: "claude" },
      launchSession: confirmed,
    });

    const authoritative = rememberTerminal(
      preserved, created.id, lateShell, undefined, NOW, "authoritative",
    );
    expect(authoritative[0].terminals[0].launch)
      .toEqual({ version: 1, profileId: "shell" });
    expect(authoritative[0].terminals[0].launchSession).toBeUndefined();
  });

  it("lets authoritative lifecycle state clear a stale Claude identity", () => {
    const created = workspace({
      terminals: [{
        id: "t-1",
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_GONE, resume: true },
      }],
    });
    const records = rememberTerminal(
      [created],
      created.id,
      summary({
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, resume: true },
      }),
      undefined,
      NOW,
      "authoritative",
    );
    expect(records[0].terminals[0].launchSession)
      .toEqual({ version: 1, resume: true });
  });

  it("replaces a fresh binding with the lifecycle-confirmed provider identity", () => {
    const created = workspace();
    const fresh = rememberTerminal(
      [created],
      created.id,
      summary({
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, resume: false },
      }),
      { version: 1, id: CHAT_GONE, resume: false },
      NOW,
    );
    expect(fresh[0].terminals[0].launchSession)
      .toEqual({ version: 1, resume: false });

    const confirmed = rememberTerminal(
      fresh,
      created.id,
      summary({
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_LIVE, resume: true },
      }),
      undefined,
      NOW,
    );
    expect(confirmed[0].terminals[0].launchSession)
      .toEqual({ version: 1, id: CHAT_LIVE, resume: true });
  });

  it("replaces a legacy confirmed-looking UUID after Claude's recovery picker", () => {
    const legacy = workspace({
      terminals: [{
        id: "t-1",
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_GONE, resume: true },
      }],
    });
    const selected = rememberTerminal(
      [legacy],
      legacy.id,
      summary({
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_LIVE, resume: true },
      }),
      undefined,
      NOW,
    );
    expect(selected[0].terminals[0].launchSession)
      .toEqual({ version: 1, id: CHAT_LIVE, resume: true });

    const latePickerResponse = rememberTerminal(
      selected,
      legacy.id,
      summary({
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, resume: true },
      }),
      undefined,
      NOW,
    );
    expect(latePickerResponse[0].terminals[0].launchSession)
      .toEqual({ version: 1, id: CHAT_LIVE, resume: true });
  });

  it("stores the arrangement with the workspace that owns it", () => {
    const created = workspace();
    const records = rememberLayout([created], created.id, "{\"version\":1}", NOW);
    expect(records[0].layout).toBe("{\"version\":1}");
    // An unchanged layout must not keep bumping updatedAt and re-saving: the
    // center skips its render and the write only when it gets the same roster.
    expect(rememberLayout(records, created.id, "{\"version\":1}", NOW)).toBe(records);
  });

  it("rewrites only the layout when a splitter frame keeps every saved terminal", () => {
    const shown = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-grok");
    const created = workspace({
      layout: serializeTerminalWorkspaceState(shown),
      terminals: [{ id: "t-grok", title: "Grok 1", cwd: "/tmp/project",
        launch: { version: 1, profileId: "grok" } }],
    });
    const other = workspace({ id: "w-other", name: "Other" });
    const moved = serializeTerminalWorkspaceState({ ...shown, maximizedPaneId: "pane-a" });
    // The caller's own ids stand in for parsing the string back.
    const records = rememberLayout([created, other], created.id, moved, NOW,
      terminalIdsInWorkspaceState(shown));
    expect(records[0].layout).toBe(moved);
    expect(records[0].terminals).toBe(created.terminals);
    expect(records[1]).toBe(other);
    // The ids decide membership exactly as the parsed layout would.
    const stray = workspace({ terminals: [
      { id: "t-stray", title: "Interface-Rust", cwd: "/tmp/Interface-Rust" }] });
    expect(rememberLayout([stray], stray.id, moved, NOW, new Set())[0].terminals).toEqual([]);
  });

  it("keeps the switcher rows while a drag changes only the active layout", () => {
    const layout = (ids: readonly string[]): string => serializeTerminalWorkspaceState(ids.reduce(
      (state, id) => openTerminalTab(state, "pane-a", id), EMPTY_TERMINAL_WORKSPACE_STATE));
    const active = workspace({ id: "w-active", name: "Active", layout: layout(["t-1"]),
      terminals: [{ id: "t-1", title: "Claude", cwd: "/tmp/project" }] });
    const parked = workspace({ id: "w-parked", name: "Parked", layout: layout(["t-2", "t-3"]),
      terminals: [{ id: "t-2", title: "Codex", cwd: "/tmp/other" }, { id: "t-3", title: "zsh", cwd: "/tmp/other" }] });
    const rows = terminalSwitcherRecords(undefined, [active, parked], active.id, ["t-1"]);
    expect(rows).toEqual([
      { id: "w-active", name: "Active", firstTerminalTitle: "Claude", openTerminalCount: 1,
        openTerminalIds: ["t-1"], terminalIds: ["t-1"], cwd: "/tmp/project" },
      { id: "w-parked", name: "Parked", firstTerminalTitle: "Codex", openTerminalCount: 2,
        openTerminalIds: ["t-2", "t-3"], terminalIds: ["t-2", "t-3"], cwd: "/tmp/other" },
    ]);
    // The parked layout is read once (savedLayoutFacts), not once per call.
    expect(rows[1].openTerminalIds).toBe(savedLayoutFacts(parked.layout).terminalIdList);
    // A splitter frame rewrites the active record's layout string and stamp.
    const dragged = { ...active, layout: `${active.layout ?? ""} `, updatedAt: "2026-08-28T00:05:00.000Z" };
    expect(terminalSwitcherRecords(rows, [dragged, parked], active.id, ["t-1"])).toBe(rows);
    const renamed = terminalSwitcherRecords(rows, [{ ...active, name: "Renamed" }, parked],
      active.id, ["t-1"]);
    expect(renamed).not.toBe(rows);
    expect(renamed[0].name).toBe("Renamed");
    expect(terminalSwitcherRecords(rows, [active, parked], active.id, ["t-1", "t-4"])[0])
      .toMatchObject({ openTerminalCount: 2, openTerminalIds: ["t-1", "t-4"] });
  });

  it("returns the same roster for lifecycle updates that change nothing saved", () => {
    const created = workspace();
    const launch = { version: 1 as const, profileId: "claude" as const };
    const binding = { version: 1 as const, id: CHAT_LIVE, resume: true };
    const records = rememberTerminal([created], created.id, summary({ launch, launchSession: binding }),
      undefined, NOW, "authoritative");
    // A resize (or fast-mode) Upsert re-files the same row: no new updatedAt,
    // no center render and no rewrite of the saved roster.
    const resized = summary({ launch, launchSession: binding, size: { columns: 132, rows: 40 },
      updatedAt: "2026-08-28T00:05:00.000Z" });
    expect(rememberTerminal(records, created.id, resized, undefined, NOW, "authoritative")).toBe(records);
    expect(rememberTerminal(records, "missing", resized, undefined, NOW)).toBe(records);
    expect(forgetTerminal(records, "missing", NOW)).toBe(records);
    const retitled = rememberTerminal(records, created.id, { ...resized, title: "Claude 2" },
      undefined, NOW, "authoritative");
    expect(retitled).not.toBe(records);
    expect(retitled[0].terminals[0].title).toBe("Claude 2");
  });

  it("drops split leftover shells that are no longer in the pane layout", () => {
    const layout = serializeTerminalWorkspaceState(
      openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-grok"),
    );
    const created = workspace({
      layout,
      terminals: [
        {
          id: "t-grok",
          title: "Grok 1",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "grok" },
        },
        { id: "t-stray", title: "Interface-Rust", cwd: "/tmp/Interface-Rust" },
        {
          id: "t-background",
          title: "Grok 2",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "grok" },
        },
      ],
    });
    const records = rememberLayout([created], created.id, layout, NOW);
    expect(records[0].terminals.map(terminal => terminal.id))
      .toEqual(["t-grok", "t-background"]);
  });
});

describe("reading an untrusted roster", () => {
  it("drops unusable records and refuses a terminal claimed twice", () => {
    const shared = { id: CHAT_LIVE, title: "Claude", cwd: "/tmp/project" };
    const records = normalizeTerminalWorkspaces([
      { id: "w-1", name: "One", createdAt: NOW(), updatedAt: NOW(), terminals: [shared] },
      { id: "w-2", name: "Two", createdAt: NOW(), updatedAt: NOW(), terminals: [shared] },
      { id: "w-1", name: "Duplicate id", createdAt: NOW(), updatedAt: NOW(), terminals: [] },
      { id: "", name: "No id", createdAt: NOW(), updatedAt: NOW(), terminals: [] },
      { id: "w-3", name: "   ", createdAt: NOW(), updatedAt: NOW(), terminals: [] },
      "not a record",
    ]);
    expect(records.map(record => record.id)).toEqual(["w-1", "w-2"]);
    // The second claim on t-1 loses; the first workspace keeps it.
    expect(records[0].terminals).toHaveLength(1);
    expect(records[1].terminals).toHaveLength(0);
    expect(normalizeTerminalWorkspaces("nonsense")).toEqual([]);
  });

  it("sanitizes nested launch data so corrupt saved panes cannot fail every restart", () => {
    const records = normalizeTerminalWorkspaces([{
      id: "w-1",
      name: "One",
      createdAt: NOW(),
      updatedAt: NOW(),
      terminals: [
        {
          id: CHAT_LIVE,
          title: "  Gemini\u0000  ",
          cwd: "/tmp/project",
          sessionKey: 42,
          launch: { version: 1, profileId: "gemini", ignored: true },
          launchSession: { version: 2, id: CHAT_LIVE, resume: false },
        },
        {
          id: CHAT_GONE,
          title: "Future",
          cwd: "/tmp/project",
          launch: { version: 2, profileId: "claude" },
        },
      ],
    }]);
    expect(records[0].terminals).toEqual([{
      id: CHAT_LIVE,
      title: "Antigravity",
      cwd: "/tmp/project",
      launch: { version: 1, profileId: "gemini" },
    }]);
  });

  it("migrates only the retired Gemini launcher's generated default titles", () => {
    const records = normalizeTerminalWorkspaces([{
      id: "w-1",
      name: "One",
      createdAt: NOW(),
      updatedAt: NOW(),
      terminals: [
        {
          id: CHAT_LIVE,
          title: "Gemini",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "gemini" },
        },
        {
          id: CHAT_GONE,
          title: "Gemini 10",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "gemini" },
        },
        {
          id: "11111111-1111-4111-8111-111111111111",
          title: "Gemini review",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "gemini" },
        },
        {
          id: "22222222-2222-4222-8222-222222222222",
          title: "Gemini 11",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "gemini" },
        },
      ],
    }]);
    expect(records[0].terminals.map(terminal => terminal.title))
      .toEqual(["Antigravity", "Antigravity 10", "Gemini review", "Gemini 11"]);
  });

  it("retains only Claude's id-less recovery-picker binding", () => {
    const records = normalizeTerminalWorkspaces([{
      id: "w-1",
      name: "One",
      createdAt: NOW(),
      updatedAt: NOW(),
      terminals: [
        {
          id: CHAT_LIVE,
          title: "Claude",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "claude" },
          launchSession: { version: 1, resume: true },
        },
        {
          id: CHAT_GONE,
          title: "Gemini",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "gemini" },
          launchSession: { version: 1, id: CHAT_GONE, resume: true },
        },
      ],
    }]);
    expect(records[0].terminals[0].launchSession)
      .toEqual({ version: 1, resume: true });
    expect(records[0].terminals[1].launchSession).toBeUndefined();
  });

  it("strips controls from a name without emptying an ordinary one", () => {
    expect(normalizeWorkspaceName("  Back\u0000end\u001b  ")).toBe("Backend");
    expect(normalizeWorkspaceName("Backend")).toBe("Backend");
    expect(normalizeWorkspaceName("   ")).toBe("");
  });
});

describe("reopening a workspace", () => {
  it("respawns only what is not already running, resuming a confirmed chat", () => {
    const record = workspace({
      layout: serializeTerminalWorkspaceState(
        attachTerminalsBalanced(
          EMPTY_TERMINAL_WORKSPACE_STATE,
          ["t-live", "t-gone", "t-shell"],
          index => `pane-${index + 1}`,
          index => `split-${index + 1}`,
        ),
      ),
      terminals: [
        {
          id: "t-live",
          title: "Running",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "claude" },
          launchSession: { version: 1, id: CHAT_LIVE, resume: false },
        },
        {
          id: "t-gone",
          title: "Claude",
          cwd: "/tmp/project",
          sessionKey: "claude#abc",
          launch: { version: 1, profileId: "claude" },
          launchSession: { version: 1, id: CHAT_GONE, resume: true },
        },
        {
          id: "t-shell",
          title: "Shell",
          cwd: "/tmp/project",
        },
      ],
    });
    const requests = reopenRequests(record, [summary({ id: "t-live" })], ids());
    expect(requests).toEqual([
      {
        id: "t-gone",
        cwd: "/tmp/project",
        title: "Claude",
        sessionKey: "claude#abc",
        launch: { version: 1, profileId: "claude" },
        // The whole point: it comes back to the chat it had, not a new one.
        launchSession: { version: 1, id: CHAT_GONE, resume: true },
      },
      { id: "t-shell", cwd: "/tmp/project", title: "Shell" },
    ]);
  });

  it("does not respawn a split leftover shell that is no longer in the layout", () => {
    const record = workspace({
      layout: serializeTerminalWorkspaceState(
        openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-gone"),
      ),
      terminals: [
        {
          id: "t-gone",
          title: "Claude",
          cwd: "/tmp/project",
          launch: { version: 1, profileId: "claude" },
        },
        { id: "t-stray", title: "Interface-Rust", cwd: "/tmp/Interface-Rust" },
      ],
    });
    expect(reopenRequests(record, [], ids()).map(request => request.id)).toEqual(["t-gone"]);
  });

  it("does not respawn an agent whose tab an earlier run closed", () => {
    // The 2026-10-01 production roster: ten closed Claude tabs in one
    // workspace all came back unseen at the next launch.
    const record = workspace({
      layout: serializeTerminalWorkspaceState(
        openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-shown"),
      ),
      terminals: [
        { id: "t-closed", title: "Research", cwd: "/tmp/project",
          launch: { version: 1, profileId: "claude" },
          launchSession: { version: 1, id: CHAT_GONE, resume: true } },
        { id: "t-shown", title: "Claude", cwd: "/tmp/project",
          launch: { version: 1, profileId: "claude" } },
        { id: "t-closed-shell", title: "Shell", cwd: "/tmp/project",
          launch: { version: 1, profileId: "shell" } },
      ],
    });
    expect(reopenRequests(record, [], ids()).map(request => request.id)).toEqual(["t-shown"]);
  });

  it("opens a legacy resumable pane fresh instead of using provider-global history", () => {
    const record = workspace({
      terminals: [{
        id: "t-legacy",
        title: "Codex",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "codex" },
        launchSession: { version: 1, resume: false },
      }],
    });
    expect(reopenRequests(record, [], () => "legacy-migrated")).toEqual([{
      id: "t-legacy",
      cwd: "/tmp/project",
      title: "Codex",
      launch: { version: 1, profileId: "codex" },
      launchSession: { version: 1, id: "legacy-migrated", resume: false },
    }]);
  });

  it("reopens a legacy Gemini pane as fresh Antigravity without obsolete binding argv", () => {
    const record = workspace({
      terminals: [{
        id: "t-antigravity",
        title: "Gemini",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "gemini" },
        launchSession: { version: 1, id: CHAT_LIVE, resume: true },
      }],
    });
    expect(reopenRequests(record, [], () => CHAT_GONE)).toEqual([{
      id: "t-antigravity",
      cwd: "/tmp/project",
      title: "Antigravity",
      launch: { version: 1, profileId: "gemini" },
    }]);
  });

  it("sends a pre-capture Claude request id through host history recovery", () => {
    const record = workspace({
      terminals: [{
        id: "t-unconfirmed",
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, id: CHAT_LIVE, resume: false },
      }],
    });
    expect(reopenRequests(record, [], () => CHAT_GONE)).toEqual([{
      id: "t-unconfirmed",
      cwd: "/tmp/project",
      title: "Claude",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: CHAT_LIVE, resume: true },
    }]);
  });

  it("opens an id-less Claude pane with the native recovery picker", () => {
    const record = workspace({
      terminals: [{
        id: "t-unconfirmed",
        title: "Claude",
        cwd: "/tmp/project",
        launch: { version: 1, profileId: "claude" },
        launchSession: { version: 1, resume: false },
      }],
    });
    expect(reopenRequests(record, [], () => CHAT_GONE)).toEqual([{
      id: "t-unconfirmed",
      cwd: "/tmp/project",
      title: "Claude",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, resume: true },
    }]);
  });

  it("returns nothing when every terminal is still running", () => {
    const record = workspace({
      terminals: [{ id: "t-1", title: "Claude", cwd: "/tmp/project" }],
    });
    expect(reopenRequests(record, [summary({ id: "t-1" })], ids())).toEqual([]);
  });
});

describe("forgetting tabs closed in an earlier run", () => {
  const claude = { version: 1, profileId: "claude" } as const;
  const shown = serializeTerminalWorkspaceState(
    openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-shown"));

  it("drops saved panes the layout no longer shows and nothing runs for", () => {
    const records = [
      workspace({ id: "w-1", layout: shown, terminals: [
        { id: "t-shown", title: "Claude", cwd: "/tmp/project", launch: claude },
        { id: "t-closed", title: "Research", cwd: "/tmp/project", launch: claude },
        { id: "t-running", title: "Parked", cwd: "/tmp/project", launch: claude },
      ] }),
      workspace({ id: "w-2", layout: shown, terminals: [
        { id: "t-shown", title: "Claude", cwd: "/tmp/project", launch: claude },
      ] }),
    ];
    const next = forgetClosedTabs(records, [summary({ id: "t-running" })], NOW);
    // A live one stays filed: the hidden sweep ends it, and its dismissal
    // forgets it, so its roster row is never lost while it still runs.
    expect(next[0].terminals.map(entry => entry.id)).toEqual(["t-shown", "t-running"]);
    expect(next[1]).toBe(records[1]);
  });

  it("keeps a roster untouched when every saved pane still returns", () => {
    const records = [workspace({ layout: shown, terminals: [
      { id: "t-shown", title: "Claude", cwd: "/tmp/project", launch: claude },
    ] })];
    expect(forgetClosedTabs(records, [], NOW)).toBe(records);
  });

  it("treats a record saved before layouts as showing every launched pane", () => {
    const records = [workspace({ terminals: [
      { id: "t-agent", title: "Claude", cwd: "/tmp/project", launch: claude },
      { id: "t-leftover", title: "Shell", cwd: "/tmp/project" },
    ] })];
    expect(forgetClosedTabs(records, [], NOW)[0].terminals.map(entry => entry.id))
      .toEqual(["t-agent"]);
  });
});

describe("workspace overview membership", () => {
  it("keeps layout-attached terminals while filing catches up and hides detached shells", () => {
    const layoutIds = terminalIdsInWorkspaceState(
      openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-grok"),
    );
    const mine = new Set(["t-grok", "t-stray", "t-background", "t-attached-shell"]);
    expect(visibleWorkspaceTerminals([
      summary({ id: "t-grok", title: "Fix terminal overlay",
        launch: { version: 1, profileId: "grok" } }),
      summary({ id: "t-stray", title: "Interface-Rust" }),
      summary({ id: "t-background", title: "- Waiting for user",
        launch: { version: 1, profileId: "grok" } }),
      summary({ id: "t-attached-shell", title: "Interface-Rust" }),
      summary({ id: "t-pending-file", title: "Claude 2",
        launch: { version: 1, profileId: "claude" } }),
      summary({ id: "t-other-workspace", title: "Other" }),
    ], mine, new Set([...layoutIds, "t-attached-shell", "t-pending-file"]))
      .map(terminal => terminal.id))
      .toEqual(["t-grok", "t-background", "t-attached-shell", "t-pending-file"]);
  });
});

describe("lookup", () => {
  it("finds a workspace by id and tolerates none being active", () => {
    const created = workspace();
    expect(workspaceById([created], created.id)).toEqual(created);
    expect(workspaceById([created], null)).toBeUndefined();
    expect(workspaceById([created], "missing")).toBeUndefined();
  });
});

describe("forgetting a workspace", () => {
  it("drops the named record and leaves the others in order", () => {
    const one = workspace({ id: "w-1", name: "One" });
    const two = workspace({ id: "w-2", name: "Two" });
    const three = workspace({ id: "w-3", name: "Three" });
    expect(forgetWorkspace([one, two, three], "w-2").map(record => record.id))
      .toEqual(["w-1", "w-3"]);
  });

  it("returns the same roster when the id is already gone", () => {
    const one = workspace({ id: "w-1", name: "One" });
    const records = [one];
    expect(forgetWorkspace(records, "missing")).toBe(records);
  });

  it("picks the next neighbor, then the previous, then none", () => {
    const records = [
      workspace({ id: "w-1", name: "One" }),
      workspace({ id: "w-2", name: "Two" }),
      workspace({ id: "w-3", name: "Three" }),
    ];
    expect(neighboringWorkspaceId(records, "w-1")).toBe("w-2");
    expect(neighboringWorkspaceId(records, "w-2")).toBe("w-3");
    expect(neighboringWorkspaceId(records, "w-3")).toBe("w-2");
    expect(neighboringWorkspaceId([records[0]], "w-1")).toBeNull();
    expect(neighboringWorkspaceId(records, "missing")).toBeNull();
  });
});

describe("workspace delete confirmation copy", () => {
  it("names an empty workspace without inventing terminals", () => {
    expect(workspaceDeleteConfirm("Backend", 0)).toEqual({
      title: "Delete Backend?",
      confirmLabel: "Delete workspace",
      description: "This removes Backend from Interface. It cannot be undone.",
    });
  });

  it("counts the terminals that will stop", () => {
    expect(workspaceDeleteConfirm("Backend", 1).description)
      .toContain("stops its 1 terminal");
    expect(workspaceDeleteConfirm("Backend", 3).description)
      .toContain("stops its 3 terminals");
  });
});

describe("failed reopens", () => {
  it("names a refused saved terminal that is still on screen, and nothing else", () => {
    const saved = workspace({
      id: "saved",
      name: "Saved",
      terminals: [
        { id: "t-shell", title: "Terminal", cwd: "/tmp/project", launch: { version: 1, profileId: "shell" } },
        { id: "t-live", title: "Live", cwd: "/tmp/project" },
        { id: "t-closed", title: "Closed", cwd: "/tmp/project" },
      ],
    });
    let layout = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-1", "t-shell");
    layout = openTerminalTab(layout, "pane-1", "t-live");
    const failures = new Map([
      ["t-shell", "That legacy saved terminal is outside Interface's current approved workspaces."],
      ["t-live", "stale"],
      ["t-closed", "stale"],
    ]);
    expect(failedReopenTerminals(saved, failures, [{ id: "t-live" }], layout)).toEqual([{
      id: "t-shell",
      title: "Terminal",
      cwd: "/tmp/project",
      profileId: "shell",
      message: "That legacy saved terminal is outside Interface's current approved workspaces.",
    }]);
    expect(failedReopenTerminals(undefined, failures, [], layout)).toEqual([]);
    expect(failedReopenTerminals(saved, new Map(), [], layout)).toEqual([]);
  });
});

describe("pane start directory", () => {
  it("starts a new pane in the project the workspace is already in, not the parent folder", () => {
    const projects = "/Users/me/Desktop/Projects";
    const project = `${projects}/Interface-Rust`;
    // The live failure: Grok opened beside Interface-Rust files, in the
    // collection folder that merely contains that project.
    expect(paneStartDirectory(projects, [project, projects], projects)).toBe(project);
    expect(paneStartDirectory(null, [project], projects)).toBe(project);
    expect(paneStartDirectory(undefined, [project, `${project}/src`], projects)).toBe(project);
  });

  it("keeps a checkout the user is actually in, and an explicit other folder", () => {
    const project = "/work/interface";
    expect(paneStartDirectory(`${project}/crates`, [project], "/work")).toBe(`${project}/crates`);
    expect(paneStartDirectory("/tmp/other", [project], project)).toBe("/tmp/other");
    expect(paneStartDirectory(project, [project], "/work")).toBe(project);
    expect(paneStartDirectory(null, [], "/work")).toBe("/work");
    expect(paneStartDirectory("  ", ["", "  "], null)).toBeNull();
  });
});

describe("shell fallback after a refused reopen", () => {
  it("retries a plain shell in its folder, then the default directory, and never a bound chat", () => {
    const shell = { id: "t-shell", cwd: "/tmp/project", title: "Terminal",
      launch: { version: 1 as const, profileId: "shell" as const } };
    expect(shellFallbackRequests(shell, "/Users/me")).toEqual([
      { id: "t-shell", cwd: "/tmp/project", title: "Terminal" },
      { id: "t-shell", cwd: "/Users/me", title: "Terminal" },
    ]);
    expect(shellFallbackRequests(shell, "/tmp/project")).toEqual([
      { id: "t-shell", cwd: "/tmp/project", title: "Terminal" },
    ]);
    expect(shellFallbackRequests(shell, null)).toHaveLength(1);
    expect(shellFallbackRequests({
      ...shell, launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, resume: true },
    }, "/Users/me")).toEqual([]);
  });
});

describe("following a moved folder", () => {
  const from = "/Users/me/Desktop/Projects/app";
  const to = "/Users/me/Developer/app";
  const record = (cwds: readonly string[]): TerminalWorkspaceRecord => ({
    id: "workspace-1",
    name: "App",
    createdAt: "2026-09-23T00:00:00.000Z",
    updatedAt: "2026-09-23T00:00:00.000Z",
    terminals: cwds.map((cwd, index) => ({ id: `terminal-${index.toString()}`, title: "Claude", cwd })),
  });

  it("moves every saved pane inside the folder and nothing beside it", () => {
    const records = [record([from, `${from}/src`, `${from}-archive`, "/Users/me/other"])];
    const moved = relocateTerminalWorkspaces(records, from, to);
    expect(moved[0].terminals.map(terminal => terminal.cwd)).toEqual([
      to, `${to}/src`, `${from}-archive`, "/Users/me/other",
    ]);
    expect(moved[0].terminals[0].id).toBe("terminal-0");
  });

  it("returns the same roster when no pane lived there", () => {
    const records = [record(["/Users/me/other"])];
    expect(relocateTerminalWorkspaces(records, from, to)).toBe(records);
  });
});
