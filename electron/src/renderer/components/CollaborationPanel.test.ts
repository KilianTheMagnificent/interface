import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CollaborationStatusDto } from "../../shared/ipc";
import { CollaborationPanel, collaborationSourceKey } from "./CollaborationPanel";

const now = "2026-08-24T16:00:00.000Z";

function status(over: Partial<CollaborationStatusDto> = {}): CollaborationStatusDto {
  return {
    sourceSessionKey: "claude#source",
    receipt: {
      version: 1,
      collaborationId: "collaboration-one",
      revision: 3,
      state: "awaiting_approval",
      mode: "consult",
      sourceEngine: "claude",
      targetEngine: "codex",
      approvalRequired: true,
      cancellationSafe: true,
      createdAt: now,
      updatedAt: now,
    },
    approval: {
      collaborationId: "collaboration-one",
      approvalId: "approval-one",
      expectedRevision: 3,
      expiresAt: "2026-08-24T16:05:00.000Z",
      mode: "consult",
      sourceEngine: "claude",
      sourceSessionKey: "claude#source",
      sourceTitle: "Source agent",
      canonicalCwd: "/work/interface",
      canonicalProjectRoot: "/work/interface",
      targetEngine: "codex",
      objective: "Review the scheduler.",
      packetHash: "a".repeat(64),
      packetPreview: "Bounded packet preview",
      packetBytes: 120,
      contextBytes: 80,
      omissions: 1,
      managedWorktree: false,
    },
    ...over,
  };
}

const handlers = {
  onRefresh: () => undefined,
  onSubmit: async () => undefined,
  onApprove: async () => undefined,
  onDeny: async () => undefined,
  onAcceptPeer: async () => undefined,
  onDiscardPeer: async () => undefined,
  onCancel: async () => undefined,
  onOpenAgent: () => undefined,
};

describe("desktop multi-agent collaboration panel", () => {
  it("renders bounded approval facts and keeps launch human gated", () => {
    const html = renderToStaticMarkup(createElement(CollaborationPanel, {
      statuses: [status()],
      sessions: [{ key: "claude#source", title: "Source", engine: "claude", status: "idle" }],
      selectedSessionKey: "claude#source",
      ...handlers,
    }));
    expect(html).toContain("Multi-agent coordination");
    expect(html).toContain("Review exact target launch");
    expect(html).toContain("Bounded packet preview");
    expect(html).toContain("Approve target");
    expect(html).toContain("Cancel safely");
  });

  it("labels held peer output untrusted and requires an explicit disposition", () => {
    const base = status();
    const html = renderToStaticMarkup(createElement(CollaborationPanel, {
      statuses: [{
        sourceSessionKey: "claude#source",
        targetSessionKey: "codex#peer",
        receipt: { ...base.receipt, revision: 8, state: "awaiting_peer_review",
          approvalRequired: false },
        peer: {
          state: "awaiting-review",
          collaborationId: "collaboration-one",
          expectedRevision: 8,
          resultId: "result-one",
          resultDigest: "b".repeat(64),
          resultBytes: 24,
          sourceSessionKey: "claude#source",
          sourceEngine: "claude",
          targetSessionKey: "codex#peer",
          targetEngine: "codex",
          objective: "Review the scheduler.",
          body: "The result remains held.",
        },
      }],
      sessions: [{ key: "claude#source", title: "Source", engine: "claude" }],
      ...handlers,
    }));
    expect(html).toContain("Untrusted peer result");
    expect(html).toContain("Accept into source");
    expect(html).toContain("Discard");
    // Discard destroys the peer's work, so it asks first, through the same
    // dialog delete-session and delete-workspace use — closed until pressed.
    expect(html).not.toContain("Discard this peer result?");
    expect(html).not.toContain('role="alertdialog"');
  });

  /* The source picker is derived, not seeded once. A mount-only seed left the
     panel with nothing selected whenever the session list landed after the
     first paint, and kept pointing at the old agent when the selected
     conversation moved — in both cases submit stayed dead. */
  describe("source agent selection", () => {
    const live = ["claude#one", "codex#two"];

    it("selects nothing while no live source exists", () => {
      expect(collaborationSourceKey([], "claude#one", null)).toBe("");
    });

    it("seeds from the list the moment it arrives, after the first paint", () => {
      expect(collaborationSourceKey(live, undefined, null)).toBe("claude#one");
      expect(collaborationSourceKey(live, null, null)).toBe("claude#one");
    });

    it("prefers the selected conversation when it is a live source", () => {
      expect(collaborationSourceKey(live, "codex#two", null)).toBe("codex#two");
    });

    it("keeps an explicit pick sticky under the same conversation", () => {
      const pick = { key: "codex#two", under: "claude#one" };
      expect(collaborationSourceKey(live, "claude#one", pick)).toBe("codex#two");
    });

    it("reseeds when the conversation changes under a sticky pick", () => {
      const pick = { key: "codex#two", under: "claude#one" };
      expect(collaborationSourceKey(live, "codex#two", pick)).toBe("codex#two");
      expect(collaborationSourceKey([...live, "acp:grok#three"], "acp:grok#three", pick))
        .toBe("acp:grok#three");
    });

    it("drops a pick whose agent is no longer live", () => {
      const pick = { key: "codex#gone", under: "claude#one" };
      expect(collaborationSourceKey(live, "claude#one", pick)).toBe("claude#one");
    });

    it("falls back to the first live source when the conversation is not one", () => {
      expect(collaborationSourceKey(live, "acp:grok#absent", null)).toBe("claude#one");
    });

    it("renders the derived key as the selected option", () => {
      const html = renderToStaticMarkup(createElement(CollaborationPanel, {
        statuses: [],
        sessions: [
          { key: "claude#one", title: "One", engine: "claude" },
          { key: "codex#two", title: "Two", engine: "codex" },
        ],
        selectedSessionKey: "codex#two",
        ...handlers,
      }));
      // A closed picker states the derived key on its trigger; the rows are
      // inside the menu, which only exists once it is opened.
      expect(html).toContain('aria-label="Source agent: Two · Codex"');
      expect(html).not.toContain("<select");
    });
  });
});
