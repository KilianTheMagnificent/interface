import { describe, expect, it } from "vitest";
import { approvalKeyTyping, approvalKeyVerdict } from "./approval-keys";
import { conversationSteerKeyAction } from "./conversation-pane-rules";
import { promptSubmitKeyAction } from "./prompt-submit";
import {
  threadComposerAnswersApproval, threadComposerPlaceholder, threadContextReadout,
  threadMenuKeyAction, threadMenuTriggerKeyAction,
} from "./thread-composer";

describe("the Threads composer's words", () => {
  it("asks for a change at rest, and says what Enter does while a turn runs", () => {
    expect(threadComposerPlaceholder({ locked: false, queueing: false, steer: false }))
      .toBe("Ask for a change or a follow-up");
    expect(threadComposerPlaceholder({ locked: false, queueing: true, steer: true }))
      .toBe("Steer now, or queue a follow-up");
    // No steer offered, no steering promised.
    expect(threadComposerPlaceholder({ locked: false, queueing: true, steer: false }))
      .toBe("Queue a follow-up");
    // A locked field promises nothing at all, a waiting request included.
    expect(threadComposerPlaceholder({ locked: true, queueing: true, steer: true }))
      .toBe("Ask for a change or a follow-up");
    expect(threadComposerPlaceholder({ locked: true, queueing: true, steer: true, waiting: true }))
      .toBe("Ask for a change or a follow-up");
  });

  it("points at the docked request while the turn waits on its owner", () => {
    for (const steer of [true, false]) {
      for (const queueing of [true, false]) {
        expect(threadComposerPlaceholder({ locked: false, queueing, steer, waiting: true }))
          .toBe("Answer above, or tell it what to do instead");
      }
    }
    expect(threadComposerPlaceholder({ locked: false, queueing: true, steer: true, waiting: false }))
      .toBe("Steer now, or queue a follow-up");
  });

  it("names the chord that really steers: Option+Enter, not Command+Enter", () => {
    const alt = { key: "Enter", shiftKey: false, altKey: true };
    const cmd = { key: "Enter", shiftKey: false, metaKey: true };
    expect(conversationSteerKeyAction(alt, true)).toBe("steer");
    // ⌘Enter is Enter: it submits, which queues while a turn runs.
    expect(conversationSteerKeyAction(cmd, true)).toBe("ignore");
    expect(promptSubmitKeyAction(cmd)).toBe("submit");
    expect(threadComposerPlaceholder({ locked: false, queueing: true, steer: true }))
      .not.toContain("⌘");
  });
});

describe("the empty field answers a docked request (owner, 2026-10-07)", () => {
  const allow = { key: "Enter", metaKey: true };
  const deny = { key: "Backspace", metaKey: true };

  it("answers from an empty field, through the docked card's own keys", () => {
    expect(threadComposerAnswersApproval({ waiting: true, draft: "", locked: false })).toBe(true);
    // The field reads the chord as the card's buttons do, typing guard aside:
    // the pane's own listener still refuses every text field.
    expect(approvalKeyVerdict(allow, false)).toBe("allow");
    expect(approvalKeyVerdict(deny, false)).toBe("deny");
    expect(approvalKeyVerdict({ key: "Enter", ctrlKey: true }, false)).toBe("allow");
    expect(approvalKeyVerdict(allow, approvalKeyTyping({ tagName: "textarea" }))).toBe(null);
    // Plain Enter, Option+Enter (steer) and Shift+⌘Enter are never a verdict.
    expect(approvalKeyVerdict({ key: "Enter" }, false)).toBe(null);
    expect(approvalKeyVerdict({ key: "Enter", metaKey: true, altKey: true }, false)).toBe(null);
    expect(approvalKeyVerdict({ key: "Enter", metaKey: true, shiftKey: true }, false)).toBe(null);
  });

  it("leaves a field with text in it exactly as it was: ⌘↵ is Enter", () => {
    expect(threadComposerAnswersApproval({ waiting: true, draft: "do this instead", locked: false }))
      .toBe(false);
    // Whitespace is text the owner typed; only the empty field answers.
    expect(threadComposerAnswersApproval({ waiting: true, draft: " ", locked: false })).toBe(false);
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false, metaKey: true })).toBe("submit");
  });

  it("answers nothing with no request waiting, or from a locked field", () => {
    expect(threadComposerAnswersApproval({ waiting: false, draft: "", locked: false })).toBe(false);
    expect(threadComposerAnswersApproval({ waiting: true, draft: "", locked: true })).toBe(false);
  });
});

describe("the + menu's keyboard", () => {
  const top = { nested: false, opens: false };
  it("moves over every row and wraps, disabled rows included", () => {
    expect(threadMenuKeyAction(4, 3, "ArrowDown", top)).toEqual({ type: "move", index: 0 });
    expect(threadMenuKeyAction(4, 0, "ArrowUp", top)).toEqual({ type: "move", index: 3 });
    expect(threadMenuKeyAction(4, 2, "Home", top)).toEqual({ type: "move", index: 0 });
    expect(threadMenuKeyAction(4, 0, "End", top)).toEqual({ type: "move", index: 3 });
    expect(threadMenuKeyAction(0, 0, "ArrowDown", top)).toBeNull();
    // Enter and Space are the row button's own click.
    expect(threadMenuKeyAction(4, 0, "Enter", top)).toBeNull();
    expect(threadMenuKeyAction(4, 0, " ", top)).toBeNull();
  });

  it("drills in and back out, and only closes from the top", () => {
    expect(threadMenuKeyAction(4, 0, "ArrowRight", { nested: false, opens: true }))
      .toEqual({ type: "open" });
    expect(threadMenuKeyAction(4, 0, "ArrowRight", top)).toBeNull();
    expect(threadMenuKeyAction(4, 1, "ArrowLeft", { nested: true, opens: false }))
      .toEqual({ type: "back" });
    expect(threadMenuKeyAction(4, 1, "Escape", { nested: true, opens: false }))
      .toEqual({ type: "back" });
    expect(threadMenuKeyAction(4, 1, "ArrowLeft", top)).toBeNull();
    expect(threadMenuKeyAction(4, 1, "Escape", top)).toEqual({ type: "close" });
    expect(threadMenuKeyAction(4, 1, "Tab", top)).toEqual({ type: "leave" });
  });

  it("opens from the button on the first row or the last", () => {
    expect(threadMenuTriggerKeyAction("ArrowDown")).toBe("first");
    expect(threadMenuTriggerKeyAction("ArrowUp")).toBe("last");
    expect(threadMenuTriggerKeyAction("Enter")).toBeNull();
  });
});

describe("the session-context readout", () => {
  it("is absent exactly where the meter chip draws nothing", () => {
    expect(threadContextReadout(undefined)).toBeNull();
    expect(threadContextReadout({ percent: null })).toBeNull();
  });

  it("carries every figure the meter's panel shows, in its order", () => {
    const readout = threadContextReadout({
      used: 125_000, size: 200_000, percent: 63, autoCompact: true,
      autoCompactThreshold: 180_000, categories: [{ name: "Tools", tokens: 30_000 }],
    }, { used: 80_000, size: 200_000, percent: 40 });
    expect(readout?.meta).toBe("63%");
    expect(readout?.high).toBe(false);
    expect(readout?.lines.map(line => line.label)).toEqual([
      "Used", "Window remaining", "Since prior report", "Tools", "Auto-compact",
      "Auto-compact threshold", "Source",
    ]);
    expect(readout?.lines.at(-1)?.value).toBe("Provider reported");
  });

  it("warns past 80% and steps down to a plain word while compacting", () => {
    expect(threadContextReadout({ percent: 85, used: 170, size: 200 })?.high).toBe(true);
    const compacting = threadContextReadout({ percent: 85, compacting: true });
    expect(compacting?.meta).toBe("Compacting");
    expect(compacting?.high).toBe(false);
    expect(compacting?.lines[0]?.value).toBe("Compacting the session window");
  });
});
