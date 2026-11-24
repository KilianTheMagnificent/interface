import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  APP_SELECT_CSS, APP_SELECT_DISMISS_EVENT, AppSelect,
  appSelectKeyAction, appSelectOptionAccessibleName,
  appSelectOptionMatches, appSelectOptionSupportingText,
  appSelectOptionVisibleMeta, appSelectOptionVisibleMetas, appSelectPopoverClasses,
  appSelectTypeaheadIndex,
  initialAppSelectIndex, moveAppSelectIndex, positionAppSelect,
  reconcileAppSelectIndex,
  type AppSelectOption,
} from "./AppSelect";

const options: AppSelectOption[] = [
  { value: "sol", label: "GPT-5.6 Sol", group: "OpenAI Codex",
    description: "Frontier coding model", meta: "OpenAI" },
  { value: "old", label: "Legacy", group: "Legacy provider", disabled: true },
  { value: "terra", label: "GPT-5.6 Terra", group: "OpenAI Codex",
    triggerLabel: "Terra · Codex", description: "Fast everyday model", meta: "OpenAI" },
];

describe("AppSelect navigation", () => {
  it("starts at the selected enabled option and falls back safely", () => {
    expect(initialAppSelectIndex(options, "terra")).toBe(2);
    expect(initialAppSelectIndex(options, "old")).toBe(0);
    expect(initialAppSelectIndex([], "", true)).toBe(0); // the custom row
    expect(initialAppSelectIndex([], "")).toBe(-1);
  });

  it("skips disabled options, wraps, and includes the optional custom row", () => {
    expect(moveAppSelectIndex(options, 0, "next")).toBe(2);
    expect(moveAppSelectIndex(options, 2, "next")).toBe(0);
    expect(moveAppSelectIndex(options, 0, "previous")).toBe(2);
    expect(moveAppSelectIndex(options, 0, "last", true)).toBe(options.length);
    expect(moveAppSelectIndex(options, options.length, "next", true)).toBe(0);
  });

  it("opens, moves, chooses, and closes with standard listbox keys", () => {
    expect(appSelectKeyAction(options, 0, "Enter", false))
      .toEqual({ type: "open", index: 0 });
    expect(appSelectKeyAction(options, 0, "ArrowDown", true))
      .toEqual({ type: "move", index: 2 });
    expect(appSelectKeyAction(options, 2, "Home", true))
      .toEqual({ type: "move", index: 0 });
    expect(appSelectKeyAction(options, 0, "End", true, true))
      .toEqual({ type: "move", index: options.length });
    expect(appSelectKeyAction(options, 2, " ", true))
      .toEqual({ type: "choose", index: 2 });
    expect(appSelectKeyAction(options, 2, "Escape", true)).toEqual({ type: "close" });
    expect(appSelectKeyAction(options, 1, "Enter", true)).toBeNull();
  });

  it("keeps the same logical row active when provider catalogues insert rows", () => {
    const before: AppSelectOption[] = [
      { value: "claude-default", label: "Automatic" },
      { value: "codex-default", label: "Automatic" },
    ];
    const after: AppSelectOption[] = [
      before[0],
      { value: "claude-opus", label: "Opus" },
      before[1],
    ];
    expect(reconcileAppSelectIndex(before, after, 1, "claude-default")).toBe(2);
    expect(reconcileAppSelectIndex(before, after, before.length,
      "claude-default", true)).toBe(after.length);
    const customBefore: AppSelectOption[] = [
      { value: "claude-default", label: "Automatic" },
      { value: "custom-claude", label: "Use custom", custom: true },
    ];
    const customAfter: AppSelectOption[] = [
      customBefore[0], { value: "claude-opus", label: "Opus" }, customBefore[1],
    ];
    // This is the value reconciliation used after Back when insertion happened
    // while the custom text-entry surface was open.
    expect(reconcileAppSelectIndex(customBefore, customAfter, 1,
      "claude-default")).toBe(2);
  });
});

describe("AppSelect typeahead", () => {
  it("searches labels, values, groups, descriptions, and metadata while skipping disabled rows", () => {
    expect(appSelectTypeaheadIndex(options, "gpt", -1)).toBe(0);
    expect(appSelectTypeaheadIndex(options, "gpt", 0)).toBe(2);
    expect(appSelectTypeaheadIndex(options, "terra", -1)).toBe(2);
    expect(appSelectTypeaheadIndex(options, "fast", -1)).toBe(2);
    expect(appSelectTypeaheadIndex(options, "openai", -1)).toBe(0);
    expect(appSelectTypeaheadIndex(options, "legacy", -1)).toBe(-1);
    expect(appSelectTypeaheadIndex(options, "anthropic", -1)).toBe(-1);
  });

  it("filters long catalogues across provider and model copy without changing typeahead", () => {
    expect(appSelectOptionMatches(options[0], "frontier")).toBe(true);
    expect(appSelectOptionMatches(options[2], "codex")).toBe(true);
    expect(appSelectOptionMatches(options[2], "TERRA")).toBe(true);
    expect(appSelectOptionMatches(options[2], "legacy")).toBe(false);
    expect(appSelectOptionMatches(options[2], "  ")).toBe(true);
  });
});

describe("positionAppSelect", () => {
  const anchor = { top: 100, right: 300, bottom: 124, left: 100, width: 200, height: 24 };

  it("opens below when it fits and keeps the trigger width", () => {
    expect(positionAppSelect(anchor, { width: 160, height: 180 }, { width: 900, height: 700 }))
      .toEqual({ top: 128, left: 100, width: 220, maxHeight: 180, side: "below" });
  });

  it("flips above when that side has more room", () => {
    const nearBottom = { ...anchor, top: 600, bottom: 624 };
    const result = positionAppSelect(nearBottom, { width: 260, height: 240 },
      { width: 900, height: 650 });
    expect(result.side).toBe("above");
    expect(result.top).toBe(356);
    expect(result.maxHeight).toBe(240);
  });

  it("clamps wide and tall menus inside a small viewport", () => {
    const edge = { ...anchor, top: 40, bottom: 64, left: 280, right: 330 };
    const result = positionAppSelect(edge, { width: 800, height: 800 },
      { width: 320, height: 220 });
    expect(result.left).toBe(8);
    expect(result.width).toBe(304);
    expect(result.maxHeight).toBeLessThanOrEqual(148);
    expect(result.top).toBeGreaterThanOrEqual(8);
  });

  it("can hang a menu leftward from its trigger's right edge, still inside the viewport", () => {
    expect(positionAppSelect(anchor, { width: 260, height: 180 }, { width: 900, height: 700 },
      undefined, "end")).toEqual({ top: 128, left: 40, width: 260, maxHeight: 180, side: "below" });
    const nearLeft = { ...anchor, left: 10, right: 40, width: 30 };
    expect(positionAppSelect(nearLeft, { width: 260, height: 180 }, { width: 900, height: 700 },
      undefined, "end").left).toBe(8);
  });
});

describe("AppSelect markup and theme", () => {
  const markup = renderToStaticMarkup(createElement(AppSelect, {
    label: "Model",
    prefix: "model",
    leading: createElement("span", { "data-test-provider-logo": "trigger" }),
    value: "terra",
    options,
    compact: true,
    allowCustom: true,
    onChange: () => undefined,
  }));

  it("ships a labelled app trigger without a native select or datalist", () => {
    expect(markup).toContain('aria-label="Model: Terra · Codex"');
    expect(markup).toContain('aria-haspopup="listbox"');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).toContain("Terra · Codex");
    expect(markup).toContain('title="Terra · Codex — Fast everyday model"');
    expect(markup).toContain("app-select-compact");
    expect(markup).toContain("app-select-leading");
    expect(markup).toContain('data-test-provider-logo="trigger"');
    expect(markup).not.toContain("<select");
    expect(markup).not.toContain("<datalist");
  });

  it("keeps friendly versioned names primary while retaining provider context", () => {
    expect(appSelectOptionAccessibleName({
      value: "gpt-default", label: "Automatic", group: "OpenAI Codex", meta: "gpt-default",
    })).toBe("Automatic, OpenAI Codex, gpt-default");
    expect(appSelectOptionAccessibleName({
      value: 'interface-model:["codex","gpt-5.6-sol"]', label: "GPT-5.6 Sol",
      group: "OpenAI Codex", meta: "gpt-5.6-sol",
    })).toBe("GPT-5.6 Sol, OpenAI Codex, gpt-5.6-sol");
    expect(appSelectOptionAccessibleName({ value: "high", label: "High" }))
      .toBe("High");
  });

  it("hides routing IDs, keeps semantic badges, and removes duplicate support copy", () => {
    const versioned = {
      value: 'interface-model:["codex","gpt-5.6-sol"]',
      label: "GPT-5.6 Sol",
      description: "Latest frontier coding model",
      meta: "gpt-5.6-sol",
    } satisfies AppSelectOption;
    expect(appSelectOptionVisibleMeta(versioned)).toBeUndefined();
    expect(appSelectOptionSupportingText(versioned, appSelectOptionVisibleMeta(versioned)))
      .toBe("Latest frontier coding model");
    expect(appSelectOptionVisibleMeta({
      value: "bypassPermissions", label: "Bypass permissions", meta: "Confirm",
    })).toBe("Confirm");
    const duplicateNames: AppSelectOption[] = [
      { value: "claude-sonnet-5", label: "Sonnet 5", group: "Anthropic Claude",
        meta: "claude-sonnet-5" },
      { value: "claude-sonnet-5-latest", label: "Sonnet 5", group: "Anthropic Claude",
        meta: "claude-sonnet-5-latest" },
    ];
    expect(appSelectOptionVisibleMeta(duplicateNames[0], duplicateNames))
      .toBe("claude-sonnet-5");
    expect(appSelectOptionAccessibleName(duplicateNames[1]))
      .toBe("Sonnet 5, Anthropic Claude, claude-sonnet-5-latest");
    expect(appSelectOptionSupportingText({
      value: "sol", label: "GPT-5.6 Sol", description: "GPT 5.6 Sol",
    }, undefined)).toBeUndefined();

    // Exact provider values remain searchable even when they are no longer chrome.
    expect(appSelectOptionMatches(versioned, "gpt-5.6-sol")).toBe(true);
    expect(appSelectTypeaheadIndex([versioned], "gpt-5.6", -1)).toBe(0);

    expect(appSelectOptionSupportingText({
      value: "x", label: "Friendly", description: "Confirm",
    }, undefined)).toBe("Confirm");
    expect(appSelectOptionSupportingText({
      value: "x", label: "Friendly", description: "Confirm",
    }, "Confirm")).toBeUndefined();
  });

  it("indexes names once and answers exactly as the pairwise scan did", () => {
    // The scan this replaced: O(n²) trim and case-fold per rebuilt option list.
    const pairwise = (option: AppSelectOption,
        list: readonly AppSelectOption[]): string | undefined => {
      const meta = option.meta?.trim();
      if (meta === undefined || meta === "") return undefined;
      const technical = meta === option.value || option.value.includes(JSON.stringify(meta));
      const labelKey = option.label.trim().toLocaleLowerCase();
      const groupKey = option.group?.trim().toLocaleLowerCase() ?? "";
      const needsDisambiguation = list.some(candidate => candidate.value !== option.value
        && candidate.label.trim().toLocaleLowerCase() === labelKey
        && (candidate.group?.trim().toLocaleLowerCase() ?? "") === groupKey);
      if (technical && !needsDisambiguation) return undefined;
      if (meta.toLocaleLowerCase() === labelKey) return undefined;
      return meta;
    };
    const list: AppSelectOption[] = [
      { value: "a", label: "Sonnet 5", group: "Claude", meta: "a" },
      { value: "b", label: " sonnet 5", group: "claude ", meta: "b" },
      { value: "c", label: "Sonnet 5", group: "Other", meta: "c" },
      { value: "d", label: "Sonnet 5", meta: "d" },
      { value: "a", label: "Sonnet 5", group: "Claude", meta: "a" },
      { value: "e", label: "Solo", meta: "e" },
      { value: "f", label: "Solo", group: "", meta: "f" },
      { value: 'm:["x","g"]', label: "G", meta: "g" },
      { value: "h", label: "Same", meta: "same" },
      { value: "i", label: "Plain", meta: "Current" },
      { value: "j", label: "Dup", meta: "j" },
      { value: "j", label: "Dup", meta: "j" },
      { value: "k", label: "a\u0000b", group: "", meta: "k" },
      { value: "l", label: "a", group: "\u0000b", meta: "l" },
      { value: "n", label: "No meta" },
    ];
    expect(appSelectOptionVisibleMetas(list))
      .toEqual(list.map(option => pairwise(option, list)));
    for (const option of list) {
      expect(appSelectOptionVisibleMeta(option, list)).toBe(pairwise(option, list));
      expect(appSelectOptionVisibleMeta(option)).toBe(pairwise(option, []));
    }
    // An option outside the list is still told apart from the listed names.
    const outsider: AppSelectOption = { value: "z", label: "Sonnet 5", group: "Claude", meta: "z" };
    expect(appSelectOptionVisibleMeta(outsider, list)).toBe(pairwise(outsider, list));
  });

  it("uses the app's existing overlay, selection, focus, and typography tokens", () => {
    expect(APP_SELECT_CSS).toContain("position:fixed");
    expect(APP_SELECT_CSS).toContain("var(--dropdown-bg)");
    expect(APP_SELECT_CSS).toContain("var(--overlay-shadow)");
    expect(APP_SELECT_CSS).toContain("var(--row-selected)");
    expect(APP_SELECT_CSS).toContain("var(--font-ui)");
    expect(APP_SELECT_CSS).toContain(".app-select-group");
    expect(APP_SELECT_CSS).toContain(".app-select-group-leading");
    expect(APP_SELECT_CSS).toContain(".provider-logo-fallback");
    expect(APP_SELECT_CSS).toContain(".app-select-search");
    expect(APP_SELECT_CSS).toContain("var(--fg-selected-secondary)");
    expect(APP_SELECT_CSS).not.toContain("linear-gradient");
  });

  it("supports an additive scoped popover class without replacing the shared root", () => {
    expect(appSelectPopoverClasses()).toBe("app-select-popover is-floating");
    expect(appSelectPopoverClasses("composer-model-popover"))
      .toBe("app-select-popover is-floating composer-model-popover");
  });

  it("exposes one app-owned dismissal event for portalled menus", () => {
    expect(APP_SELECT_DISMISS_EVENT).toBe("interface:app-select-dismiss");
  });

  it("aligns rich option text on one heading baseline with explicit line metrics", () => {
    expect(APP_SELECT_CSS)
      .toContain("grid-template-columns:var(--icon-size-sm) minmax(0, 1fr)");
    expect(APP_SELECT_CSS).toContain(".app-select-option-heading");
    expect(APP_SELECT_CSS).toContain("align-items:baseline");
    expect(APP_SELECT_CSS).toContain("min-height:32px");
    expect(APP_SELECT_CSS).toContain("line-height:var(--lh-ui)");
    expect(APP_SELECT_CSS).toContain("line-height:var(--lh-tight)");
    expect(APP_SELECT_CSS).not.toContain("minmax(0, 1fr) auto");
  });

  it("renders a disabled trigger as a real disabled button", () => {
    const disabled = renderToStaticMarkup(createElement(AppSelect, {
      label: "Effort", value: "", options: [], disabled: true,
      onChange: () => undefined,
    }));
    expect(disabled).toContain("disabled=\"\"");
    expect(disabled).toContain('aria-label="Effort: Select…"');
  });
});
