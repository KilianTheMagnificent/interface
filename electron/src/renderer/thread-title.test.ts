import { describe, expect, it } from "vitest";
import { shortPromptTitle, threadDisplayTitle } from "./thread-title";

describe("a thread's short title (owner, 2026-11-15)", () => {
  it("takes the prompt's first line, collapses its white space and cuts it on a word near 60 characters", () => {
    expect(shortPromptTitle("Fix   the\tlogin test")).toBe("Fix the login test");
    expect(shortPromptTitle("\n\n  First line here\nsecond line")).toBe("First line here");
    const long = "burst write plan.md Draft the release checklist for atlas and then check every link in it";
    const short = shortPromptTitle(long);
    expect(short).toBe("burst write plan.md Draft the release checklist for atlas…");
    expect([...short].length).toBeLessThanOrEqual(61);
    // Exactly 60 characters is kept whole.
    const sixty = "a".repeat(29) + " " + "b".repeat(30);
    expect(shortPromptTitle(sixty)).toBe(sixty);
    // A word longer than the budget is cut where the budget ends.
    expect(shortPromptTitle("x".repeat(90))).toBe(`${"x".repeat(60)}…`);
    // Trailing punctuation before the cut goes with it.
    expect(shortPromptTitle(`${"word ".repeat(11)}tail, and more words after the cut`))
      .toBe(`${"word ".repeat(11)}tail…`);
  });

  it("keeps a title the owner set or Claude generated as it is, and shortens one still its prompt", () => {
    const long = "burst write plan.md Draft the release checklist for atlas and then check every link in it";
    expect(threadDisplayTitle(long, "user")).toBe(long);
    expect(threadDisplayTitle(long, "generated")).toBe(long);
    expect(threadDisplayTitle(long, "prompt")).toBe(shortPromptTitle(long));
    // No record yet: a provider's own summary stays; a prompt fallback is shortened.
    expect(threadDisplayTitle(long, undefined, true)).toBe(long);
    expect(threadDisplayTitle(long, undefined, false)).toBe(shortPromptTitle(long));
    expect(threadDisplayTitle("   ", "prompt")).toBe("   ");
  });
});
