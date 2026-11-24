import { describe, expect, it } from "vitest";
import { promptSubmitKeyAction } from "./prompt-submit";

describe("promptSubmitKeyAction", () => {
  it("sends on Enter and keeps Shift+Enter as a newline", () => {
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false })).toBe("submit");
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: true })).toBe("newline");
  });

  it("does not require a modifier to send", () => {
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false, metaKey: false,
      ctrlKey: false })).toBe("submit");
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false, metaKey: true }))
      .toBe("submit");
  });

  it("ignores IME composition confirmation and non-Enter keys", () => {
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false, isComposing: true }))
      .toBe("ignore");
    expect(promptSubmitKeyAction({
      key: "Enter", shiftKey: false, nativeEvent: { isComposing: true },
    })).toBe("ignore");
    expect(promptSubmitKeyAction({
      key: "Enter", shiftKey: false, nativeEvent: { keyCode: 229 },
    })).toBe("ignore");
    expect(promptSubmitKeyAction({ key: "a", shiftKey: false })).toBe("ignore");
  });

  it("treats Alt+Enter as a newline so a second break key remains", () => {
    expect(promptSubmitKeyAction({ key: "Enter", shiftKey: false, altKey: true }))
      .toBe("newline");
  });
});
