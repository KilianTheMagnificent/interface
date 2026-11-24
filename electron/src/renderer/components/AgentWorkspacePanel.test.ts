import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  emptyAgentControlResponse, type AgentControlResponse, type AgentDecision, type AgentFiles,
  type AgentProfile, type AgentRequest,
} from "../../shared/agents";
import type { ConvItem } from "../conversation-reducer";
import {
  AGENT_ASKS_BEFORE, AGENT_MEMORY_CONFLICT, AGENT_MEMORY_TOO_LARGE, AGENT_PAST_THREAD, AGENT_PROVIDER_NOTE,
  AgentComputerView, AgentMemoryView, AgentPastThreadNotice, AgentWorkspacePanel, agentThreadPane,
  AGENT_PERMISSION_HELP,
  dismissRestartNotice, restartNoticeOpen, saveMemoryDraft, sendFirstThreadMessage, shouldMarkRead,
  type AgentRequestResult, type RestartLatches,
} from "./AgentWorkspacePanel";
import { Conversation, type ConversationProps } from "./Conversation";

const noop = (): void => undefined;

const agent = (over: Partial<AgentProfile> = {}): AgentProfile => ({
  id: "a1", name: "Research", engine: "claude", model: "", effort: "", instructions: "Cite primary sources.",
  color: "teal", ...over,
});

const response = (over: Partial<AgentControlResponse> = {}): AgentControlResponse => ({
  ...emptyAgentControlResponse(), agents: [agent()], ...over,
});
const ok = (over: Partial<AgentControlResponse> = {}): AgentRequestResult => ({ ok: true, response: response(over) });
const refused = (error: string): AgentRequestResult => ({ ok: false, error });

const files = (over: Partial<AgentFiles> = {}): AgentFiles => ({
  home: "/fixture/agents/Research", exists: true, markerOk: true,
  instructions: { text: "Cite primary sources.", sha256: "i-sha", tooLarge: false },
  instructionsConflict: false,
  memory: { text: "Remember the brief.", sha256: "old-sha", tooLarge: false },
  ...over,
});

const threaded = (interrupted = false, key = "claude#t-2"): AgentControlResponse => response({ threads: { a1: {
  current: { sessionKey: key, startedAtMs: 1788958900000, status: "idle", interrupted },
  past: [{ sessionKey: "claude#t-1", startedAtMs: 1788958000000, endedAtMs: 1788958500000 }],
  lastReadAtMs: 0, lastActivityAtMs: 1788958900000,
} }, runs: [{ id: "r1", routineId: "x", agentId: "a1", name: "Morning brief", scheduledAtMs: 0, updatedAtMs: 0,
  status: "waiting", sessionKey: "codex#run-1", detail: "", prompt: "", cwd: "/f", began: true, profile: agent() }],
  pendingApprovals: [{ agentId: "a1", sessionKey: "codex#run-1", approvalId: "ask-1", tool: "Bash",
    summary: "git push", reason: "risky:git-push", askedAtMs: 1 }] });

const panel = (over: Partial<Parameters<typeof AgentWorkspacePanel>[0]> = {}): string =>
  renderToStaticMarkup(createElement(AgentWorkspacePanel, {
    agent: agent(), response: response(), pending: false, error: null,
    shownSessionKey: null, visible: true,
    onCommand: async () => true, onRequest: async () => ok(), onOpenTask: noop, onOpenThread: noop,
    onShowThread: noop, onPickDirectory: async () => null,
    children: createElement("p", { className: "fixture-conversation" }, "SHOWN CONVERSATION"),
    ...over,
  }));

const memoryView = (over: Partial<Parameters<typeof AgentMemoryView>[0]> = {}): string =>
  renderToStaticMarkup(createElement(AgentMemoryView, {
    agent: agent(), files: files(), draft: "", messages: {}, pending: false,
    onDraft: noop, onSave: noop, onReveal: noop, onRecreate: noop, onUseInterfaceInstructions: noop,
    ...over,
  }));

describe("AgentWorkspacePanel tabs", () => {
  it("opens on Conversation, ahead of Memory, Routines, Runs and Computer", () => {
    const html = panel();
    const tabs = [...html.matchAll(/<button type="button" aria-pressed="(true|false)">([^<]+)<\/button>/gu)]
      .map(match => [match[2], match[1]]);

    expect(tabs).toEqual([["Conversation", "true"], ["Memory", "false"], ["Routines", "false"],
      ["Runs", "false"], ["Computer", "false"]]);
  });

  it("asks for the first message when the agent has no thread and nothing else is shown", () => {
    const html = panel();

    expect(html).toContain('<label for="agent-thread-first-a1"');
    expect(html).toContain(">Message Research</label>");
    expect(html).toContain('<textarea id="agent-thread-first-a1"');
    expect(html).toMatch(/<button type="submit"[^>]*disabled=""[^>]*>Send<\/button>/u);
    expect(html).not.toContain("SHOWN CONVERSATION");
    expect(html).not.toContain(">New conversation</button>");
  });

  it("mounts the current conversation with New conversation and past conversations", () => {
    const html = panel({ response: threaded(), shownSessionKey: "claude#t-2" });

    expect(html).toContain("SHOWN CONVERSATION");
    expect(html).toContain(">New conversation</button>");
    expect(html).toContain("Past conversations");
    // "Thread" means a Threads item; a teammate keeps a conversation (review C2).
    expect(html).not.toContain(">New thread</button>");
    expect(html).not.toContain("Past threads");
    expect(html).not.toContain("agent-thread-first-a1");
  });

  it("wires the Computer tab to the asks-before line and the provider note", () => {
    const html = panel({ initialView: "computer" });

    expect(html).toContain(AGENT_ASKS_BEFORE);
    expect(html).toContain(AGENT_PROVIDER_NOTE);
    expect(html).not.toContain("agent-permission-badge");
  });

  it("names a provider mode on the thread bar and in the Computer tab", () => {
    const bypass = { ...threaded(), permissionModes: { a1: "bypass" as const } };
    const html = panel({ response: bypass, shownSessionKey: "claude#t-2" });
    expect(html).toMatch(/class="badge badge-quiet agent-permission-badge" data-permission="bypass"[^>]*>Bypass<\/span>/u);
    expect(panel({ response: threaded(), shownSessionKey: "claude#t-2" })).not.toContain("agent-permission-badge");
    const computer = panel({ response: { ...response(), permissionModes: { a1: "auto" } }, initialView: "computer" })
      .replaceAll("&#x27;", "'");
    expect(computer).toContain(AGENT_PERMISSION_HELP.auto);
    expect(computer).not.toContain(AGENT_ASKS_BEFORE);
  });
});

describe("what the Thread tab shows", () => {
  it("shows whichever session App focused, and jumps to the thread only when asked", () => {
    // A run's pending approval opened from the tray, and an older task from navigation.
    expect(agentThreadPane("claude#t-2", "codex#run-1")).toBe("other");
    expect(agentThreadPane(undefined, "codex#older-task")).toBe("other");
    expect(agentThreadPane("claude#t-2", "claude#t-2")).toBe("current");
    expect(agentThreadPane(undefined, null)).toBe("first-message");
    expect(agentThreadPane("claude#t-2", null)).toBe("open-current");
  });

  it("keeps a run session reached from the tray on screen", () => {
    const html = panel({ response: threaded(), shownSessionKey: "codex#run-1" });

    expect(html).toContain("SHOWN CONVERSATION");
    expect(html).not.toContain(">New conversation</button>");
    expect(html).not.toContain("agent-thread-first-a1");
  });

  it("keeps an older task opened from navigation on screen when the agent has no thread", () => {
    const html = panel({ shownSessionKey: "codex#older-task" });

    expect(html).toContain("SHOWN CONVERSATION");
    expect(html).not.toContain("agent-thread-first-a1");
  });

  it("offers the conversation when one exists but nothing is shown", () => {
    const html = panel({ response: threaded(), shownSessionKey: null });

    expect(html).toContain(">Open conversation</button>");
    expect(html).not.toContain("SHOWN CONVERSATION");
  });

  it("leaves no effect that moves the pane on mount", () => {
    const source = readFileSync(new URL("./AgentWorkspacePanel.tsx", import.meta.url), "utf8");
    expect(source).not.toMatch(/useEffect\(\(\) => \{\s*if \(view === "thread" && currentKey/u);
    expect(source).not.toMatch(/useEffect\([^;]*onOpenThread\(/u);
  });
});

describe("the restart notice", () => {
  it("stays once seen until dismissed or a message is sent, and comes back on a later restart", () => {
    const latches: RestartLatches = new Map();

    expect(restartNoticeOpen(latches, "t", true, undefined)).toBe(true);
    // markRead cleared `interrupted`; the owner has not dismissed or sent.
    expect(restartNoticeOpen(latches, "t", false, undefined)).toBe(true);
    // The owner sent a message in the thread.
    expect(restartNoticeOpen(latches, "t", false, "echo-1")).toBe(false);
    // A later restart latches again.
    expect(restartNoticeOpen(latches, "t", true, "echo-1")).toBe(true);
    dismissRestartNotice(latches, "t");
    expect(restartNoticeOpen(latches, "t", true, "echo-1")).toBe(false);
    expect(restartNoticeOpen(latches, "t", false, "echo-1")).toBe(false);
  });

  it("renders on the thread and survives the read that clears `interrupted`", () => {
    const key = "claude#restart-render";
    const notice = "Interface restarted while this agent was working. Check the conversation before continuing.";
    const first = panel({ response: threaded(true, key), shownSessionKey: key });
    const afterRead = panel({ response: threaded(false, key), shownSessionKey: key });
    const afterSend = panel({ response: threaded(false, key), shownSessionKey: key, threadLastSendId: "echo-9" });

    expect(first).toContain(notice);
    expect(first).toContain('aria-label="Dismiss restart notice"');
    expect(afterRead).toContain(notice);
    expect(afterSend).not.toContain(notice);
  });

  it("marks read only while the owner can actually see the Thread tab", () => {
    const seen = { view: "thread", pane: "current", visible: true } as const;
    expect(shouldMarkRead({ ...seen, status: "unread", interrupted: false })).toBe(true);
    expect(shouldMarkRead({ ...seen, status: "idle", interrupted: true })).toBe(true);
    expect(shouldMarkRead({ ...seen, visible: false, status: "unread", interrupted: true })).toBe(false);
    expect(shouldMarkRead({ ...seen, view: "memory", status: "unread", interrupted: false })).toBe(false);
    expect(shouldMarkRead({ ...seen, status: "working", interrupted: false })).toBe(false);
    // The Thread tab showing something else is not opening the thread: a run, an
    // older task or a past thread restored at launch, or only an Open thread button.
    for (const pane of ["other", "open-current", "first-message"] as const) {
      expect(shouldMarkRead({ ...seen, pane, status: "unread", interrupted: true }), pane).toBe(false);
    }
  });
});

describe("the first thread message", () => {
  it("sends startAgentThread with the typed text", async () => {
    const calls: AgentRequest[] = [];
    const result = await sendFirstThreadMessage("a1", "  Summarize the project.\n", async command => {
      calls.push(command); return ok({ sessionKey: "claude#t-1" });
    }, "request-1");

    expect(result?.ok).toBe(true);
    expect(calls).toEqual([{ kind: "startAgentThread", agentId: "a1", text: "Summarize the project.",
      requestId: "request-1" }]);
  });

  it("returns the host's refusal, and sends nothing for a blank draft", async () => {
    const calls: AgentRequest[] = [];
    const onRequest = async (command: AgentRequest): Promise<AgentRequestResult> => {
      calls.push(command); return refused("Claude is not signed in.");
    };

    expect(await sendFirstThreadMessage("a1", "Hello", onRequest, "request-2"))
      .toEqual({ ok: false, error: "Claude is not signed in." });
    expect(await sendFirstThreadMessage("a1", "   ", onRequest, "request-3")).toBeNull();
    expect(calls).toHaveLength(1);
  });
});

describe("the Memory tab", () => {
  it("reloads and says so when MEMORY.md changed since it was opened", async () => {
    const loaded = files();
    const changed = files({ memory: { text: "The agent's own note.", sha256: "new-sha", tooLarge: false } });
    const calls: AgentRequest[] = [];
    const result = await saveMemoryDraft("a1", loaded.memory!, "My edit", async command => {
      calls.push(command);
      return command.kind === "readAgentFiles" ? ok({ files: changed })
        : refused("MEMORY.md changed since it was opened.");
    });

    expect(calls).toEqual([
      { kind: "saveAgentMemory", agentId: "a1", expectedSha256: "old-sha", text: "My edit" },
      { kind: "readAgentFiles", agentId: "a1" },
    ]);
    expect(result).toEqual({ files: changed, notice: { text: AGENT_MEMORY_CONFLICT, alert: true } });
    expect(AGENT_MEMORY_CONFLICT).toBe("MEMORY.md changed since you opened it. Reloaded.");

    const html = memoryView({ files: changed, draft: changed.memory!.text, messages: { save: result.notice! } });
    expect(html).toContain("MEMORY.md changed since you opened it. Reloaded.");
    expect(html).toContain('<textarea id="agent-memory-a1"');
    expect(html).toContain("/fixture/agents/Research");
    expect(html).toContain(">Show in Finder</button>");
  });

  it("keeps a non-conflict refusal and the draft through the follow-up read", async () => {
    const loaded = files();
    const result = await saveMemoryDraft("a1", loaded.memory!, "My edit", async command =>
      command.kind === "readAgentFiles" ? ok({ files: loaded }) : refused("The agent folder is missing."));

    expect(result).toEqual({ files: null, notice: { text: "The agent folder is missing.", alert: true } });
    const html = memoryView({ draft: "My edit", messages: { save: result.notice! } });
    expect(html).toMatch(/>Save memory<\/button><p role="alert"[^>]*>The agent folder is missing\.<\/p>/u);
  });

  it("refuses a draft over 256 KiB of UTF-8 before sending it", async () => {
    const calls: AgentRequest[] = [];
    // 87,382 three-byte characters: well under 256,000 characters, over 262,144 bytes.
    const result = await saveMemoryDraft("a1", files().memory!, "€".repeat(87_382), async command => {
      calls.push(command); return ok();
    });

    expect(result).toEqual({ files: null, notice: { text: AGENT_MEMORY_TOO_LARGE, alert: true } });
    expect(AGENT_MEMORY_TOO_LARGE).toBe("Memory is larger than 256 KiB");
    expect(calls).toEqual([]);
    const fits = await saveMemoryDraft("a1", files().memory!, "€".repeat(87_381), async command => {
      calls.push(command); return ok({ files: files() });
    });
    expect(fits.notice).toEqual({ text: "MEMORY.md saved.", alert: false });
    expect(calls).toHaveLength(1);
  });

  it("shows each control's refusal beside that control", () => {
    const html = memoryView({
      files: files({ exists: false, memory: null, instructions: null }),
      messages: { recreate: { text: "Could not create the folder.", alert: true } },
    });
    expect(html).toMatch(/>Recreate folder<\/button><p role="alert"[^>]*>Could not create the folder\.<\/p>/u);
  });

  it("offers Interface's instructions over an edited file, and recreates a missing folder", () => {
    const conflict = memoryView({ files: files({ instructionsConflict: true,
      instructions: { text: "Edited outside Interface.", sha256: "x", tooLarge: false } }) });
    expect(conflict).toContain("Edited outside Interface.");
    expect(conflict).toContain("Cite primary sources.");
    expect(conflict).toContain(">Use Interface&#x27;s version</button>");

    const missing = memoryView({ files: files({ exists: false, memory: null, instructions: null }) });
    expect(missing).toContain(">Recreate folder</button>");

    const large = memoryView({ files: files({ memory: { text: "", sha256: "big", tooLarge: true } }) });
    expect(large).not.toContain("<textarea");
    expect(large).toContain("read-only");
  });
});

describe("a past thread", () => {
  it("reads as read-only with no composer and no verdict controls", () => {
    const approval: ConvItem = { kind: "approval", id: "old-ask", tool: "Bash", summary: "npm test", input: {} };
    const props = {
      items: [approval], thinking: false, live: true,
      config: { effort: null, models: [], canSetModel: false, effortLevels: [], showEffort: false,
        canSetEffort: false, pendingModel: false, pendingEffort: false, error: null },
      onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop, onApprove: noop, onDeny: noop,
      approvals: {}, activeApprovalIds: new Set<string>(),
      composer: createElement(AgentPastThreadNotice, { hasCurrent: true, onBack: noop }),
    } as ConversationProps;
    const html = renderToStaticMarkup(createElement(Conversation, props));

    expect(AGENT_PAST_THREAD).toBe("This is a past conversation. It's read-only.");
    expect(html).toContain("This is a past conversation. It&#x27;s read-only.");
    expect(html).toContain(">Back to current conversation</button>");
    expect(html).not.toContain('class="conv-composer"');
    expect(html).not.toContain("<textarea");
    expect(html).not.toContain("data-approval-key");
    expect(html).not.toMatch(/>\s*Allow\s*</u);

    const noCurrent = renderToStaticMarkup(createElement(AgentPastThreadNotice, { hasCurrent: false, onBack: noop }));
    expect(noCurrent).toContain(">Back to conversation</button>");
  });

  it("is wired read-only in every conversation App mounts", () => {
    const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
    const tile = readFileSync(new URL("./SessionTile.tsx", import.meta.url), "utf8");
    expect(tile).toContain("const pastThread = agentPastThread(agentStore, key);");
    expect(tile).toContain("composer={pastThread === null ? undefined");
    expect(tile).toContain("activeApprovalIds={pastThread !== null ? NO_APPROVAL_IDS");
    expect(tile).toContain("queueItems={pastThread === null ? tileQueue : NO_QUEUE}");
    expect(tile).toContain("approvalGate={agentApprovalGateFor(agentStore, key, agentStartKnownKeys)}");
    expect(app).toContain('visible={surface === "agent" && centerView === "sessions" && !settingsOpen && documentVisible}');
  });
});

describe("the Computer tab", () => {
  const decision = (over: Partial<AgentDecision>): AgentDecision => ({
    atMs: 1788958910000, sessionKey: "claude#t-1", approvalId: "ap", tool: "Bash", summary: "Run build",
    facts: { kind: "shell", command: "npm run build", cwd: "/fixture/project" }, decision: "allowed",
    reason: "", outcome: "sent", ...over,
  });

  it("lists the home, granted folders, recent decisions and what it asks before", () => {
    const html = renderToStaticMarkup(createElement(AgentComputerView, {
      agent: agent({ home: "/fixture/agents/Research", folders: ["/fixture/project", "/fixture/docs"] }),
      decisions: [
        decision({ approvalId: "a", summary: "Run build" }),
        decision({ approvalId: "b", tool: "WebFetch", summary: "Fetch docs", decision: "asked", atMs: 1788958920000 }),
        decision({ approvalId: "c", summary: "Push branch", decision: "ownerAllowed" }),
        decision({ approvalId: "d", summary: "Delete cache", decision: "ownerDenied" }),
        decision({ approvalId: "e", summary: "Unknown tool", decision: "failedClosed" }),
      ],
      pending: false, messages: {}, onAddFolder: noop, onRemoveFolder: noop, onReveal: noop,
    }));

    expect(html).toContain("/fixture/agents/Research");
    expect(html).toContain("/fixture/project");
    expect(html).toContain("/fixture/docs");
    expect(html).toContain(">Add folder</button>");
    expect(html).toContain('aria-label="Remove /fixture/docs"');
    expect(html).toContain(AGENT_ASKS_BEFORE);
    expect(AGENT_ASKS_BEFORE).toBe(
      "Requests approval for recognized risky actions. Scripts and provider-native tools can bypass this policy; it is not a security boundary.");
    expect(html).toContain(AGENT_PROVIDER_NOTE);
    for (const label of ["Allowed", "Asked you", "You allowed", "You denied", "Refused on error"]) {
      expect(html).toContain(`>${label}</span>`);
    }
    expect(html).toContain("Fetch docs");
    // Newest first.
    expect(html.indexOf("Fetch docs")).toBeLessThan(html.indexOf("Run build"));
  });

  it("says why decisions or Show in Finder failed", () => {
    const html = renderToStaticMarkup(createElement(AgentComputerView, {
      agent: agent(), decisions: "unavailable", pending: false,
      messages: { decisions: { text: "Decision log is unreadable.", alert: true },
        reveal: { text: "Finder could not open the folder.", alert: true } },
      onAddFolder: noop, onRemoveFolder: noop, onReveal: noop,
    }));
    expect(html).toContain("Decision log is unreadable.");
    expect(html).toMatch(/>Show in Finder<\/button><p role="alert"[^>]*>Finder could not open the folder\.<\/p>/u);
  });
});
