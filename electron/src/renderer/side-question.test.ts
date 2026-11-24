import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { SideQuestionPanel } from "./components/SideQuestionPanel";
import {
  bringBackSide, sideAnswer, sideQuestionCommand, sideQuestionOffered, sideStatusText,
} from "./side-question";

/* Side questions (protocol 67, T3 section 12): `/btw <question>` asks a hidden
   fork of the thread; the right column shows the question and the streaming
   answer, with Bring back (a quote in the thread's draft) and Close. Rust
   forks, hides and removes; these pin the words, the parse and the quote. */

const side = { key: "claude#side-1", question: "Which test covers the  login timeout?", askedAt: 1 };
const noop = (): void => undefined;

describe("a side question's answer", () => {
  it("streams the main agent's text, ignoring subagents, and settles when the side thread is idle", () => {
    const events: EngineEvent[] = [{ kind: "status.changed", status: "working" }];
    expect(sideAnswer(events).state).toBe("answering");
    expect(sideAnswer([]).state).toBe("asking");
    events.push({ kind: "message.delta", text: "It's " },
      { kind: "message.delta", text: "noise", agentKey: "sub-1" },
      { kind: "message.delta", text: "login.test.ts, the second case." });
    expect(sideAnswer(events)).toEqual({ state: "answering", text: "It's login.test.ts, the second case." });
    events.push({ kind: "status.changed", status: "idle" });
    const answered = sideAnswer(events);
    expect(answered.state).toBe("answered");
    expect(sideStatusText(answered)).toBe("Answered. Nothing was added to the thread.");
    const failed = sideAnswer([{ kind: "error", message: "Not logged in", retryable: false }]);
    expect(failed).toEqual({ state: "failed", text: "", error: "Not logged in" });
    expect(sideStatusText(failed)).toBe("Failed: Not logged in");
  });

  it("is asked with /btw in a thread's composer, never sent as a prompt", () => {
    expect(sideQuestionCommand("/btw which test covers this?")).toBe("which test covers this?");
    expect(sideQuestionCommand("  /BTW\nwhat about\ntwo lines")).toBe("what about\ntwo lines");
    expect(sideQuestionCommand("/btw")).toBe("");
    expect(sideQuestionCommand("/btwx no")).toBeNull();
    expect(sideQuestionCommand("fix /btw later")).toBeNull();
    expect([sideQuestionOffered("claude"), sideQuestionOffered("codex"), sideQuestionOffered("acp:grok")])
      .toEqual([true, true, false]);
  });

  it("comes back into the thread's draft as a quote led by its question", () => {
    const write = bringBackSide("Next, fix it.", side, "It's login.test.ts.\nThe second case.");
    expect(write?.text).toBe(
      "Next, fix it.\n\n> Side question: Which test covers the login timeout?\n>\n> It's login.test.ts.\n> The second case.\n\n");
    expect(write?.cursor).toBe(write?.text.length);
    expect(bringBackSide("", side, "  ")).toBeNull();
  });
});

describe("the side question panel", () => {
  it("shows the question, the answer so far and Bring back only once it is answered", () => {
    const panel = (state: "answering" | "answered"): string => renderToStaticMarkup(createElement(SideQuestionPanel, {
      side, answer: { state, text: "It's login.test.ts." }, closing: false, onBringBack: noop, onClose: noop,
    }));
    const answering = panel("answering");
    expect(answering).toContain('aria-label="Side question" data-side-question="claude#side-1"');
    expect(answering).toContain("Which test covers the  login timeout?");
    expect(answering).toContain("Answering…");
    expect(answering).toMatch(/disabled="" title="Add the answer to this thread&#x27;s draft[^"]*">Bring back</u);
    const answered = panel("answered");
    expect(answered).toContain("It&#x27;s login.test.ts.");
    // Formatted as a reply is: Markdown, never raw HTML.
    const formatted = renderToStaticMarkup(createElement(SideQuestionPanel, {
      side, answer: { state: "answered", text: "It's **login.test.ts**:\n\n- the `timeout` case\n- <b>raw</b>" },
      closing: false, onBringBack: noop, onClose: noop,
    }));
    expect(formatted).toContain('<div class="conv-markdown">');
    expect(formatted).toContain("<strong>login.test.ts</strong>");
    expect(formatted).toMatch(/<li>[^]*<code[^>]*>timeout<\/code>/u);
    expect(formatted).not.toContain("<b>raw</b>");
    expect(formatted).toContain("&lt;b&gt;raw&lt;/b&gt;");
    expect(answered).toMatch(/<button type="button" class="side-panel-btn is-primary" title="[^"]*">Bring back</u);
    expect(answered).toContain(">Close</button>");
  });
});
