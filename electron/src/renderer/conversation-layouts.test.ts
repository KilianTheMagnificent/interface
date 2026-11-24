import { describe, expect, it } from "vitest";
import type { SessionSummary } from "../shared/engine";
import {
  CONVERSATION_SURFACES, conversationSurfaceFor, layoutFor, reconcileConversationLayouts,
  type ConversationLayouts,
} from "./conversation-layouts";
import { EMPTY_WORKSPACE_STATE, openWorkspaceKey, switchWorkspaceKey } from "./workspace";

const summary = (sessionId: string, kind?: SessionSummary["kind"]): SessionSummary => ({
  engine: sessionId.startsWith("codex") ? "codex" : "claude", sessionId, title: sessionId,
  cwd: "/work", status: "working", cloud: false, origin: "app",
  firstSeen: "2026-10-06T00:00:00.000Z", lastActivity: "2026-10-06T00:00:00.000Z",
  archived: false, ...(kind === undefined ? {} : { kind }),
});
const EMPTY: ConversationLayouts = { chat: EMPTY_WORKSPACE_STATE, agent: EMPTY_WORKSPACE_STATE };

describe("conversation layouts", () => {
  it("has Chat and Agent slots only; Code shows an empty layout", () => {
    expect(CONVERSATION_SURFACES).toEqual(["chat", "agent"]);
    const layouts = { ...EMPTY, chat: switchWorkspaceKey(EMPTY_WORKSPACE_STATE, "claude#a") };
    expect(layoutFor(layouts, "chat")).toBe(layouts.chat);
    expect(layoutFor(layouts, "agent")).toBe(layouts.agent);
    expect(layoutFor(layouts, "code")).toBe(EMPTY_WORKSPACE_STATE);
  });

  it("opens a session on the surface that owns it, never on Code", () => {
    expect(conversationSurfaceFor(summary("claude-a", "agent"), "code")).toBe("agent");
    expect(conversationSurfaceFor(summary("claude-a", "code"), "agent")).toBe("chat");
    expect(conversationSurfaceFor(summary("claude-a"), "code")).toBe("chat");
    // Not listed yet: stay on the conversation surface on screen, or leave Code for Chat.
    expect(conversationSurfaceFor(undefined, "agent")).toBe("agent");
    expect(conversationSurfaceFor(undefined, "chat")).toBe("chat");
    expect(conversationSurfaceFor(undefined, "code")).toBe("chat");
  });

  it("keeps an adversarial-review pair open across registry pushes", () => {
    // The host starts the builder and reviewer with no session kind: Chat's.
    const builder = summary("claude-builder");
    const reviewer = summary("codex-reviewer");
    const opened: ConversationLayouts = { ...EMPTY,
      chat: openWorkspaceKey(openWorkspaceKey(EMPTY_WORKSPACE_STATE, "claude#claude-builder"),
        "codex#codex-reviewer") };
    let layouts = opened;
    for (const push of [[builder, reviewer], [reviewer, builder, summary("claude-other")]]) {
      layouts = reconcileConversationLayouts(layouts, push);
    }
    expect(layouts).toBe(opened);
    expect(layouts.chat.openKeys).toEqual(["claude#claude-builder", "codex#codex-reviewer"]);
  });

  it("drops a pane whose session left its surface's listing, and nothing else", () => {
    const layouts: ConversationLayouts = {
      chat: openWorkspaceKey(switchWorkspaceKey(EMPTY_WORKSPACE_STATE, "claude#chat-a"), "claude#gone"),
      agent: switchWorkspaceKey(EMPTY_WORKSPACE_STATE, "claude#task"),
    };
    const next = reconcileConversationLayouts(layouts,
      [summary("chat-a"), summary("task", "agent")]);
    expect(next.chat.openKeys).toEqual(["claude#chat-a"]);
    expect(next.agent).toBe(layouts.agent);
    // A chat key restored into Agent's slot does not belong there.
    const crossed = reconcileConversationLayouts(
      { ...EMPTY, agent: switchWorkspaceKey(EMPTY_WORKSPACE_STATE, "claude#chat-a") },
      [summary("chat-a")]);
    expect(crossed.agent.openKeys).toEqual([]);
  });

  it("hands back the same record for an unchanged push and skips empty slots", () => {
    expect(reconcileConversationLayouts(EMPTY, [summary("a")])).toBe(EMPTY);
    const layouts = { ...EMPTY, chat: switchWorkspaceKey(EMPTY_WORKSPACE_STATE, "claude#a") };
    expect(reconcileConversationLayouts(layouts, [summary("a"), summary("b", "agent")])).toBe(layouts);
  });
});
