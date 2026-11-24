import { describe, expect, it } from "vitest";
import {
  draftHasContent, stashChord, stashDraftOf, stashOtherItems, stashPathItems, stashRowWords,
} from "./draft-stash-view";

const NOW = Date.UTC(2026, 10, 15, 12);

describe("stashed drafts in the renderer", () => {
  it("names each row by its first line and age, or by what it holds", () => {
    expect(stashRowWords({ id: "s", preview: "Rename the page", at: NOW - 5 * 60_000, items: 2 }, NOW))
      .toEqual({ label: "Rename the page", detail: "5m ago · 2 attachments" });
    expect(stashRowWords({ id: "s", preview: "", at: NOW, items: 1 }, NOW))
      .toEqual({ label: "1 attachment", detail: "just now" });
  });

  it("hands Rust the text and plain references, never a thumbnail or backend bytes", () => {
    const draft = stashDraftOf("Fix it", [
      { id: "f", kind: "file", label: "a.png", value: "/tmp/a.png",
        previewUrl: "data:image/png;base64,AAAA" },
      { id: "d", kind: "diff", label: "Changes", value: "/repo", safeContent: "+secret" },
    ]);
    expect(draft).toEqual({ text: "Fix it", items: [
      { id: "f", kind: "file", label: "a.png", value: "/tmp/a.png" },
      { id: "d", kind: "diff", label: "Changes", value: "/repo" },
    ] });
    expect(draftHasContent("  ", [])).toBe(false);
    expect(draftHasContent("", [{ id: "u", kind: "url", label: "u", value: "https://e.test" }])).toBe(true);
  });

  it("splits a restored draft into paths to grant again and chips to draw", () => {
    const items = [
      { id: "f", kind: "file" as const, label: "a.png", value: "/tmp/a.png" },
      { id: "c", kind: "chat" as const, label: "Fix the header", value: "codex#b" },
    ];
    expect(stashPathItems(items).map(item => item.id)).toEqual(["f"]);
    expect(stashOtherItems(items).map(item => item.id)).toEqual(["c"]);
  });

  it("is ⌘S alone", () => {
    const key = { key: "s", metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
    expect(stashChord(key)).toBe(true);
    expect(stashChord({ ...key, key: "S" })).toBe(true);
    expect(stashChord({ ...key, shiftKey: true })).toBe(false); // ⌘⇧S settles a thread
    expect(stashChord({ ...key, metaKey: false })).toBe(false);
  });
});
