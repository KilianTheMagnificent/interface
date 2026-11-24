import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const source = readFileSync(fileURLToPath(new URL("./App.tsx", import.meta.url)), "utf8");

describe("collaboration draft publication", () => {
  it("serializes restored-session advisories behind one bounded host tail", () => {
    const tail = source.indexOf(
      "const collaborationDraftPublishTail = useRef<Promise<void>>(Promise.resolve())",
    );
    const publish = source.indexOf("const publishCollaborationDraftState", tail);
    const invoke = source.indexOf("window.iface.setCollaborationDraftState", publish);
    const advance = source.indexOf(
      "collaborationDraftPublishTail.current = next.catch(() => undefined)",
      invoke,
    );
    expect(tail).toBeGreaterThan(-1);
    expect(publish).toBeGreaterThan(tail);
    expect(invoke).toBeGreaterThan(publish);
    expect(advance).toBeGreaterThan(invoke);
    expect(source).not.toContain("useRef<Map<SessionKey, Promise<void>>>");
  });

  it("seeds each revision from the clock so a reload never restarts below the host's mark", () => {
    const publish = source.indexOf("const publishCollaborationDraftState");
    const invoke = source.indexOf("window.iface.setCollaborationDraftState", publish);
    const body = source.slice(publish, invoke);
    expect(body).toContain(
      "nextDraftRevision(collaborationDraftRevisions.current.get(key), Date.now())",
    );
    expect(body).not.toMatch(/get\(key\) \?\? 0\) \+ 1/);
  });

  it("skips keystrokes the host already knows about but never a send's republish", () => {
    expect(source).toContain("createDraftStore({ onWrite: publishCollaborationDraftWrite })");
    const write = source.slice(source.indexOf("const publishCollaborationDraftWrite"),
      source.indexOf("const [conversationDrafts]"));
    expect(write).toContain(
      "if (!draftWriteNeedsPublish(collaborationDraftLastEmpty.current.get(key), draft)) return;");
    // The outbound lane's publish after a send reaches the host unfiltered.
    expect(source).toMatch(
      /publishDraft: key => \{\n {6}publishCollaborationDraftState\(key, conversationDrafts\.get\(key\)\);/u);
    const publish = source.slice(source.indexOf("const publishCollaborationDraftState"),
      source.indexOf("window.iface.setCollaborationDraftState"));
    expect(publish).toContain("if (!isCollaborationSourceKey(key)) return;");
    expect(publish).toContain("collaborationDraftLastEmpty.current.set(key, draft.length === 0);");
  });
});
