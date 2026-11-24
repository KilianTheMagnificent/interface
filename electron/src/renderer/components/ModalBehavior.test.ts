import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  MODAL_FOCUSABLE_SELECTOR,
  focusPastTrigger,
  logicalListFocusIndex,
  modalKeyAction,
  modalKeyMayAct,
  rovingTabKeyAction,
  useModalFocus,
} from "./ModalBehavior";

describe("modalKeyAction", () => {
  it("wraps focus and respects the direction used to enter the modal", () => {
    expect(modalKeyAction("Tab", 0, false, 3)).toEqual({ type: "focus", index: 1 });
    expect(modalKeyAction("Tab", 2, false, 3)).toEqual({ type: "focus", index: 0 });
    expect(modalKeyAction("Tab", 0, true, 3)).toEqual({ type: "focus", index: 2 });
    expect(modalKeyAction("Tab", -1, true, 3)).toEqual({ type: "focus", index: 2 });
  });

  it("keeps a control-free modal focused and can temporarily suppress Escape", () => {
    expect(modalKeyAction("Tab", -1)).toEqual({ type: "focus-container" });
    expect(modalKeyAction("Escape", -1)).toEqual({ type: "dismiss" });
    expect(modalKeyAction("Escape", -1, false, 0, false)).toBeNull();
    expect(modalKeyAction("Enter", 0, false, 1)).toBeNull();
  });

  it("names every key it can act on, so the hook skips its scan for the rest", () => {
    for (const key of ["a", " ", "Enter", "Backspace", "ArrowDown", "Escape", "Tab", "Home"]) {
      for (const escapeEnabled of [true, false]) {
        for (const [active, count] of [[-1, 0], [0, 3], [2, 3]] as const) {
          const acts = modalKeyAction(key, active, false, count, escapeEnabled) !== null;
          expect(modalKeyMayAct(key, escapeEnabled)).toBe(acts);
        }
      }
    }
  });
});

describe("rovingTabKeyAction", () => {
  it("wraps arrows and supports Home and End", () => {
    expect(rovingTabKeyAction("ArrowRight", 2, 3)).toEqual({ index: 0 });
    expect(rovingTabKeyAction("ArrowLeft", 0, 3)).toEqual({ index: 2 });
    expect(rovingTabKeyAction("ArrowDown", 0, 3)).toEqual({ index: 1 });
    expect(rovingTabKeyAction("Home", 2, 3)).toEqual({ index: 0 });
    expect(rovingTabKeyAction("End", 0, 3)).toEqual({ index: 2 });
    expect(rovingTabKeyAction("Enter", 0, 3)).toBeNull();
  });
});

describe("modal focus lifecycle helpers", () => {
  it("chooses the nearest surviving row after a removal", () => {
    expect(logicalListFocusIndex(2, 4)).toBe(2);
    expect(logicalListFocusIndex(3, 3)).toBe(2);
    expect(logicalListFocusIndex(0, 0)).toBe(-1);
  });

  it("excludes disabled form controls and captures and restores the opener", () => {
    expect(MODAL_FOCUSABLE_SELECTOR).toContain("button:not([disabled])");
    const source = useModalFocus.toString();
    expect(source).toContain("document.activeElement");
    expect(source).toContain("priorFocus");
    expect(source).toContain("targetToRestore");
  });
});

describe("focusPastTrigger", () => {
  const read = (name: string): string =>
    readFileSync(new URL(`./${name}`, import.meta.url), "utf8");

  it("is the one copy the three popovers hand their Tab-out to", () => {
    // Three near-identical ~20-line copies drifted: AppSelect's filter had no
    // [hidden] clause, so a hidden option outside its own popover could take
    // the caret. One export, one selector, one filter.
    for (const name of ["EffortBar.tsx", "PromptContextPicker.tsx", "AppSelect.tsx"]) {
      const source = read(name);
      expect(source).toContain('import { focusPastTrigger } from "./ModalBehavior"');
      expect(source).toContain("focusPastTrigger(trigger, backwards,");
      expect(source).not.toContain("[contenteditable='true']");
    }
    const behavior = read("ModalBehavior.ts");
    expect(behavior).toContain('node.closest("[hidden], [inert]") === null');
    // Not the modal selector: a Tab out lands on a document control, so
    // contenteditable counts and the hidden-input exclusion does not apply.
    expect(behavior).toContain("[contenteditable='true']");
    expect(MODAL_FOCUSABLE_SELECTOR).not.toContain("[contenteditable='true']");
  });

  it("leaves focus alone when there is no trigger to step past", () => {
    expect(focusPastTrigger(null, false, null)).toBeNull();
  });
});
