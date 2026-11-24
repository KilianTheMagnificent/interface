import { describe, expect, it } from "vitest";
import { terminalCardPreview } from "./terminal-card-preview";

describe("terminal card previews", () => {
  it("keeps recent task text while removing a known agent's idle chrome", () => {
    expect(terminalCardPreview([
      "Old line", "Updated the retry handler", "Tests passed", "────────",
      "› Ask Codex to do anything", "gpt-5.6 · /work/interface", "/work/interface", "90% context left",
    ], "/work/interface", "codex")).toEqual(["Updated the retry handler", "Tests passed"]);
  });
  it("omits empty cards and repeated directory metadata", () => {
    expect(terminalCardPreview(["", "›", "~/project", "/Users/person/project"],
      "/Users/person/project", "codex")).toEqual([]);
  });
  it("preserves task prompts, model discussion, and shell output", () => {
    expect(terminalCardPreview(["› Fix the auth handler", "gpt-5.6 supports this task"], "/repo", "codex"))
      .toEqual(["› Fix the auth handler", "gpt-5.6 supports this task"]);
    for (const profile of [undefined, "shell", "custom"])
      expect(terminalCardPreview(["› Ask Codex to do anything", "90% context left"], "/repo", profile))
        .toEqual(["› Ask Codex to do anything", "90% context left"]);
  });
  it("keeps wrapped agent replies together and omits Codex startup notices", () => {
    expect(terminalCardPreview([
      "› Tell me something fun", "• Shuffle a deck of cards, and you have probably created an order",
      "  that has never existed before.", "  A small act of cosmic originality.",
      "• You have 2 usage limit resets available. Run /usage to use one.",
      "› Ask Codex to do anything", "  gpt-6-astra xhigh · ~/Desktop/Projects",
    ], "/Users/person/Desktop/Projects", "codex")).toEqual([
      "› Tell me something fun",
      "• Shuffle a deck of cards, and you have probably created an order that has never existed before. A small act of cosmic originality.",
    ]);
    expect(terminalCardPreview([
      "╭─────────────────────╮", "│ >_ OpenAI Codex (v0.153.4) │", "│ model: gpt-6-astra │",
      "│ directory: ~/Desktop/Projects │", "╰─────────────────────╯",
      "  Tip: Try the Desktop app. Run 'codex app' or visit",
      "  https://chatgpt.com/codex?app-landing-page=true", "› Ask Codex to do anything",
    ], "/Users/person/Desktop/Projects", "codex")).toEqual([]);
  });

  it("bounds work and collapses adjacent duplicate output", () => {
    expect(terminalCardPreview(["Outside the window", ...Array<string>(40).fill("")], "/repo")).toEqual([]);
    expect(terminalCardPreview(["First", "Latest", "Latest"], "/repo")).toEqual(["First", "Latest"]);
  });
});

describe("terminalCardPreview for the agent overview", () => {
  it("returns as many lines as the caller asks for", () => {
    const lines = ["● Read src/a.ts", "● Bash npm test", "  ⎿  ✓ 18 passed", "✻ Wiring…"];
    // An indented result line stays with the step it belongs to.
    expect(terminalCardPreview(lines, "/tmp/p", "claude", 3))
      .toEqual(["● Read src/a.ts", "● Bash npm test ⎿  ✓ 18 passed", "✻ Wiring…"]);
    expect(terminalCardPreview(lines, "/tmp/p", "claude")).toHaveLength(2);
  });

  it("skips Claude Code's prompt box and mode line", () => {
    expect(terminalCardPreview([
      "● Done. Install uses the signed DMG.",
      "╭──────────────╮",
      "│ >            │",
      "╰──────────────╯",
      "  ⏵⏵ accept edits on (shift+tab to cycle)",
      "  ⏸ plan mode on (shift+tab to cycle)",
    ], "/tmp/p", "claude", 3)).toEqual(["● Done. Install uses the signed DMG."]);
  });
});

describe("terminalCardPreview joins a provider's wrapped prose", () => {
  it("reads a lower-case line on from an unfinished sentence", () => {
    expect(terminalCardPreview([
      "The CI runner caps request bodies", "at 10 MB, so the 20 MB fixture", "never finishes.",
      "Lower it to 8 MB.",
    ], "/work/interface", "codex", 3)).toEqual([
      "The CI runner caps request bodies at 10 MB, so the 20 MB fixture never finishes.",
      "Lower it to 8 MB.",
    ]);
    // A plain shell keeps its rows as they are.
    expect(terminalCardPreview(["total 8", "drwxr-xr-x  4 me"], "/w", "shell", 3))
      .toEqual(["total 8", "drwxr-xr-x  4 me"]);
  });
});
