import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { attachmentCount, keptPromptDraft, keptPromptLead, keptPromptPaths, keptPromptShows } from "./PromptRecoveryPanel";

describe("the kept prompt in the composer's top edge (owner, 2026-11-15)", () => {
  it("shows only a message with something to recover, never one delivered and answered", () => {
    // The receipt stays stored either way; only the line's visibility changes.
    expect(keptPromptShows("accepted", false)).toBe(false);
    expect(keptPromptShows("accepted", true)).toBe(true);
    expect(keptPromptShows("unknown", false)).toBe(true);
    expect(keptPromptShows("unsent", false)).toBe(true);
  });

  it("says what happened to the message in plain words", () => {
    expect(keptPromptLead("unsent")).toBe("A message you wrote earlier was not sent");
    expect(keptPromptLead("unknown")).toBe("A message you wrote earlier may not have finished");
    expect(keptPromptLead("accepted")).toBe("A message you wrote earlier may not have finished");
    expect(attachmentCount(1)).toBe("1 attachment");
    expect(attachmentCount(3)).toBe("3 attachments");
  });

  it("puts the message back after anything already typed, and attaches its files and folders again", () => {
    expect(keptPromptDraft("", "Add a rollback step")).toBe("Add a rollback step");
    expect(keptPromptDraft("  \n", "Add a rollback step")).toBe("Add a rollback step");
    expect(keptPromptDraft("Also this\n", "Add a rollback step")).toBe("Also this\n\nAdd a rollback step");
    expect(keptPromptPaths([
      { id: "a", kind: "file", label: "plan.md", value: "/w/plan.md" },
      { id: "b", kind: "folder", label: "src", value: "/w/src" },
      { id: "c", kind: "diff", label: "Review", value: "diff", safeContent: "…" },
    ])).toEqual([{ path: "/w/plan.md" }, { path: "/w/src", directory: true }]);
  });

  it("sits inside the composer card, not as a strip across the pane", () => {
    const tile = readFileSync(new URL("./SessionTile.tsx", import.meta.url), "utf8");
    expect(tile).toContain("composerTop={composerTop}");
    expect(tile).not.toContain("<PromptRecoveryPanel");
    const composer = readFileSync(new URL("./ConversationComposer.tsx", import.meta.url), "utf8");
    expect(composer).toMatch(/className=\{`thread-composer[\s\S]{0,200}\{props\.composerTop\}\s*\{banners\}/u);
    const css = readFileSync(new URL("./ThreadComposer.css", import.meta.url), "utf8");
    expect(css).toMatch(/\.kept-prompt \{[^}]*border-bottom:var\(--border-w\) solid var\(--border\)/u);
  });
});
