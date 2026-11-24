import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SessionKind } from "../shared/engine";
import { COMPOSER_QUIET_FOCUS, Composer } from "./components/Composer";
import {
  composerChatPermissionMode, composerErrorField,
  composerKindEngines, composerRememberedPermissionMode, preferredEffort, seedLaunchForm,
  startingPermissionMode, type EngineChoices, type EngineOption,
} from "./composer-model";
import {
  applyPromptContextMention, IDLE_PROMPT_CONTEXT_MENTION_STATE,
  PROMPT_CONTEXT_MENTION_MAX_QUERY, PROMPT_CONTEXT_PICKER_CSS, promptContextCaret,
  promptContextMention, promptContextMentionChoice, promptContextMentionHint,
  promptContextMentionKeyAction, promptContextMentionOptions,
  promptContextMentionView, PromptContextMentionMenu,
  retainedPromptContextDismissal, trackedPromptContextMention,
  type PromptContextMentionView,
} from "./components/PromptContextPicker";
import type { ModelPreset } from "./model-presets";
import type { PromptContextCapabilities } from "./prompt-context";

/* Rules this file owns, all of them findings from the GUI audit:
     H2  the launch card stated one instruction three times at once
     L7  the prompt showed keyboard focus by caret only
     L10 the sheet floated in the middle of a mostly-empty pane
     M6  off-token radius literals in the launch toolbar
     @   typed inline context — the Cursor-parity gap in section 4

   Same technique as ui-components.test.ts: vite runs these in node, which
   cannot mount a component but renders one to static markup perfectly well, so
   the markup and the CSS the card actually ships are facts here. */

const CHOICES: EngineChoices = {
  models: ["claude-opus-5"],
  modelOptions: [
    { value: "claude-opus-5", displayName: "Opus 5",
      supportsEffort: false, effortLevels: [] },
  ],
  efforts: [],
  permissionModes: ["default"],
  chat: true,
};

const CODEX_CHOICES: EngineChoices = {
  models: ["gpt-5.1-codex"],
  modelOptions: [
    { value: "gpt-5.1-codex", displayName: "GPT-5.1 Codex",
      supportsEffort: false, effortLevels: [] },
  ],
  efforts: [],
  permissionModes: ["default"],
};

const ENGINES: EngineOption[] = [{ id: "claude" }, { id: "codex" }];
const choicesFor = (engine: string): EngineChoices =>
  engine === "codex" ? CODEX_CHOICES : CHOICES;

const card = (over: {
  kind?: SessionKind;
  model?: string;
  modelPresets?: readonly ModelPreset[];
  contextCapabilities?: PromptContextCapabilities;
  contextRequests?: boolean;
  teammate?: string;
} = {}): string => renderToStaticMarkup(createElement(Composer, {
  open: true,
  ...(over.teammate === undefined ? {} : { teammate: over.teammate }),
  ...(over.kind === undefined ? {} : { kind: over.kind }),
  defaults: {
    engine: "claude", cwd: "/w/repo", prompt: "",
    model: over.model ?? "", effort: "", permissionMode: "default",
  },
  engines: ENGINES,
  choicesFor,
  ...(over.modelPresets === undefined ? {} : { modelPresets: over.modelPresets }),
  ...(over.contextCapabilities === undefined
    ? {} : { contextCapabilities: over.contextCapabilities }),
  ...(over.contextRequests === true ? { onContextRequest: () => undefined } : {}),
  onLaunch: () => undefined,
  onBrowse: () => Promise.resolve(null),
  onClose: () => undefined,
}));

const composerCss = readFileSync(new URL("./components/Composer.css", import.meta.url), "utf8");

/** Markup without the stylesheet, so a class assertion cannot be satisfied by
 *  the rule that styles it. */
const bodyOf = (markup: string): string =>
  markup.replace(/<style[^>]*>[\s\S]*?<\/style>/, "");

const count = (haystack: string, needle: string): number =>
  haystack.split(needle).length - 1;

describe("H2 — the launch card says each instruction once", () => {
  const body = bodyOf(card());

  it("keeps the heading to what this surface is, not to what to do next", () => {
    const intro = /<div class="composer-intro">([\s\S]*?)<\/div>/.exec(body)?.[1] ?? "";
    expect(intro).toContain("Interface");
    expect(intro).toContain("Drop a file path, a traceback, or a rough idea");
    expect(intro).not.toContain("keep steering while the agent works");
    // Both directives used to live here as well as on their own controls.
    expect(intro).not.toMatch(/choose a model/i);
    expect(intro).not.toMatch(/describe the outcome/i);
  });

  it("states the prompt instruction only in the prompt's own placeholder", () => {
    expect(body).toContain("placeholder=\"Give Interface a task\"");
    expect(count(body, "Give Interface a task")).toBe(1);
  });

  it("leaves the model instruction to the picker, and 'why is this disabled' to the submit", () => {
    // The picker still names itself for AT and still shows its placeholder.
    expect(body).toContain("aria-label=\"Model: Choose model\"");
    const submit = /<button[^>]*class="composer-submit"[^>]*>/.exec(body)?.[0] ?? "";
    expect(submit).toContain("disabled");
    expect(submit).toContain("aria-label=\"Choose a model before starting the chat\"");
    // Every remaining mention of the directive is inside that one disabled
    // button, where it answers a different question and is never on screen
    // unless the pointer is on it.
    expect(count(body, "Choose a model")).toBe(count(submit, "Choose a model"));
  });

  it("treats the default sentinel as no model, and names Start chat once one is chosen", () => {
    const missing = /<button[^>]*class="composer-submit"[^>]*>/
      .exec(bodyOf(card({ model: "default" })))?.[0] ?? "";
    expect(missing).toContain("disabled");
    expect(missing).toContain("aria-label=\"Choose a model before starting the chat\"");
    const ready = /<button[^>]*class="composer-submit"[^>]*>/
      .exec(bodyOf(card({ model: "claude-opus-5" })))?.[0] ?? "";
    expect(ready).not.toContain("disabled");
    expect(ready).toContain("aria-label=\"Start chat\"");
    expect(bodyOf(card({ model: "claude-opus-5" })))
      .toContain("placeholder=\"Give Interface a task\"");
  });
});

describe("launch effort follows the selected model instead of sitting empty", () => {
  const withEffort: EngineChoices = {
    models: ["claude-opus-5"],
    modelOptions: [
      { value: "claude-opus-5", displayName: "Opus 5",
        supportsEffort: true, effortLevels: ["low", "medium", "high"] },
    ],
    efforts: ["low", "medium", "high"],
    permissionModes: ["default"],
  };

  it("prefers a remembered concrete level, then the model's first advertised one", () => {
    expect(preferredEffort(["low", "medium", "high"], "high", "low")).toBe("high");
    expect(preferredEffort(["low", "medium", "high"], "", "medium")).toBe("medium");
    expect(preferredEffort(["low", "medium", "high"], "default")).toBe("low");
  });

  it("seeds an empty effort field so the picker never opens as a blank Effort", () => {
    const seeded = seedLaunchForm({
      engine: "claude", cwd: "/w/repo", prompt: "",
      model: "claude-opus-5", effort: "", permissionMode: "default",
    }, withEffort, { effort: "high", permissionMode: "default" });
    expect(seeded.effort).toBe("high");
    expect(seedLaunchForm({
      engine: "claude", cwd: "/w/repo", prompt: "",
      model: "claude-opus-5", effort: "", permissionMode: "default",
    }, withEffort).effort).toBe("low");
  });

  it("seeds the last-used access mode so a new chat does not drop Automatic", () => {
    const claudeModes: EngineChoices = {
      ...withEffort,
      permissionModes: ["default", "acceptEdits", "plan", "auto", "dontAsk",
        "bypassPermissions"],
    };
    expect(seedLaunchForm({
      engine: "claude", cwd: "/w/repo", prompt: "",
      model: "claude-opus-5", effort: "", permissionMode: "default",
    }, claudeModes, { effort: "high", permissionMode: "auto" }).permissionMode)
      .toBe("auto");
    expect(seedLaunchForm({
      engine: "claude", cwd: "/w/repo", prompt: "",
      model: "claude-opus-5", effort: "", permissionMode: "default",
    }, withEffort, { effort: "high", permissionMode: "auto" }).permissionMode)
      .toBe("default");
  });
});

describe("L7 — keyboard focus on the prompt is visible, and is not the resting state", () => {
  const css = composerCss;

  it("gives the prompt surface the app's own field-focus edge", () => {
    expect(css).toContain(".composer-box:has(.composer-ta:focus-visible)");
    const rule = /\.composer-box:has\(\.composer-ta:focus-visible\)\s*\{([^}]*)\}/
      .exec(css)?.[1] ?? "";
    expect(rule).toContain("border-color: var(--focus)");
    expect(rule).toContain("box-shadow: 0 0 0 1px var(--focus)");
  });

  it("still refuses to light the whole compound surface for the toolbar controls", () => {
    expect(css).not.toContain(".composer-box:focus-within");
  });

  /* The regression this replaces: `autoFocus` is script focus, and script focus
     on a <textarea> matches :focus-visible in this app's own Chromium — checked
     in a real Electron 43.3.0 BrowserWindow — so the accent edge was on from the
     moment New Chat opened and stayed on. A border the widest surface in the app
     wears by default is not a focus indicator, it is chrome that claims focus. */
  it("opens the card with the caret in the prompt but without the ring", () => {
    // The attribute is gone: the browser's own autofocus is the thing that lit
    // the rule above, and it cannot be asked for a quiet focus.
    expect(bodyOf(card())).not.toContain("autofocus");
    expect(COMPOSER_QUIET_FOCUS.focusVisible).toBe(false);
    expect(/focus\(COMPOSER_QUIET_FOCUS\)/.test(Composer.toString())).toBe(true);
    // And the plain :focus variant, which the quiet focus would still match,
    // is not left behind anywhere in the sheet.
    expect(css).not.toMatch(/\.composer-ta:focus\)/);
  });
});

describe("L10 — the launch surface shares the live composer's working edge", () => {
  it("fills the panel and docks the prompt at the foot", () => {
    const css = composerCss;
    expect(css).toContain(".workspace-composer-panel { justify-content: stretch; align-items: stretch; }");
    expect(css).toMatch(/\.composer-card\s*\{[^}]*height:\s*100%/);
    expect(css).toContain(".composer-dock {");
    expect(css).toContain(".composer-intro {");
    expect(css).toMatch(/\.composer-intro\s*\{[^}]*flex:\s*1 1 auto/);
  });

  it("keeps recent model history out of the resting launch surface", () => {
    const body = bodyOf(card({
      modelPresets: [
        { engine: "codex", model: "gpt-5.1-codex", effort: "high",
          permissionMode: "default" },
        { engine: "claude", model: "claude-opus-5", effort: "default",
          permissionMode: "default" },
      ],
    }));
    expect(body).not.toContain("composer-recent");
    expect(body).not.toContain("Recent models");
  });

  it("shows no rail at all on a window that has launched nothing", () => {
    const body = bodyOf(card());
    expect(body).not.toContain("composer-recent");
    expect(body).not.toContain("Recent models");
  });
});

/* Cursor parity: typing "@" attaches context without leaving the keyboard.
   The picker and its capability gating already existed; only the trigger was
   missing, so these rules are about the trigger being exact — it must fire on
   a reference and stay out of the way of ordinary prose, and taking a row must
   remove what was typed as the trigger and nothing else. */
describe("@ — what counts as a typed context reference", () => {
  it("opens on an @ at a word boundary and reads the query up to the caret", () => {
    expect(promptContextMention("@", 1)).toEqual({ start: 0, end: 1, query: "" });
    expect(promptContextMention("fix @fi", 7)).toEqual({ start: 4, end: 7, query: "fi" });
    expect(promptContextMention("(@fo", 4)).toEqual({ start: 1, end: 4, query: "fo" });
    expect(promptContextMention("line one\n@di", 12))
      .toEqual({ start: 9, end: 12, query: "di" });
  });

  it("leaves prose alone: an address, a handle mid-word, and a second @", () => {
    // The whole reason the trigger is boundary-anchored: these are not
    // references, and firing here would later delete text the user typed.
    expect(promptContextMention("mail git@host", 13)).toBeNull();
    expect(promptContextMention("@a@b", 4)).toBeNull();
    expect(promptContextMention("no trigger here", 15)).toBeNull();
  });

  it("stops once the token is prose: whitespace, an overlong run, a caret inside a word", () => {
    expect(promptContextMention("@fi le", 6)).toBeNull();
    expect(promptContextMention(`@${"x".repeat(PROMPT_CONTEXT_MENTION_MAX_QUERY)}`,
      PROMPT_CONTEXT_MENTION_MAX_QUERY + 1)).not.toBeNull();
    expect(promptContextMention(`@${"x".repeat(PROMPT_CONTEXT_MENTION_MAX_QUERY + 1)}`,
      PROMPT_CONTEXT_MENTION_MAX_QUERY + 2)).toBeNull();
    // Caret between "@" and "file": completing here would eat the word.
    expect(promptContextMention("@file", 1)).toBeNull();
    expect(promptContextMention("@fi", 9)).toBeNull();
  });

  it("keeps an Escape dismissal on its own token and forgets it anywhere else", () => {
    // Still typing inside the token Escape dismissed: stay shut.
    expect(retainedPromptContextDismissal("@fil", 4, 0)).toBe(0);
    // Caret left it, the token is gone, or a different "@" is under the caret:
    // the next trigger has to open, including one that reuses offset 0.
    expect(retainedPromptContextDismissal("@fil now", 8, 0)).toBeNull();
    expect(retainedPromptContextDismissal("plain text", 10, 0)).toBeNull();
    expect(retainedPromptContextDismissal("done @f", 7, 0)).toBeNull();
    expect(retainedPromptContextDismissal("@fil", null, 0)).toBeNull();
    expect(retainedPromptContextDismissal("@fil", 4, null)).toBeNull();
  });

  it("removes exactly the trigger and puts the caret where it was", () => {
    const draft = "review @fil";
    const mention = promptContextMention(draft, draft.length);
    if (mention === null) throw new Error("the trigger did not fire on a typed reference");
    expect(applyPromptContextMention(draft, mention))
      .toEqual({ prompt: "review ", caret: 7 });
    // Mid-draft, both sides survive byte-for-byte — including the space the
    // user typed before "@" and the one after the token.
    const inline = promptContextMention("a @f b", 4);
    if (inline === null) throw new Error("the trigger did not fire mid-draft");
    expect(applyPromptContextMention("a @f b", inline))
      .toEqual({ prompt: "a  b", caret: 2 });
  });
});

describe("@ — it offers only what this engine can actually attach", () => {
  const capabilities: PromptContextCapabilities = {
    file: { available: true },
    folder: { available: true },
    diff: { available: true },
    chat: { available: false, unavailableReason: "Prior chat is not connected yet." },
  };

  it("drops kinds the caller reports as unavailable instead of listing dead rows", () => {
    expect(promptContextMentionOptions(capabilities, 0, "").map(row => row.kind))
      .toEqual(["file", "folder", "diff"]);
    // Nothing available means no menu at all, which is what keeps "@" inert in
    // prose for a caller that has wired no context surface.
    expect(promptContextMentionOptions({}, 0, "")).toEqual([]);
  });

  it("filters by kind and by the words of the label", () => {
    expect(promptContextMentionOptions(capabilities, 0, "f").map(row => row.kind))
      .toEqual(["file", "folder"]);
    expect(promptContextMentionOptions(capabilities, 0, "diff").map(row => row.kind))
      .toEqual(["diff"]);
    expect(promptContextMentionOptions({ chat: { available: true } }, 0, "chat")
      .map(row => row.label)).toEqual(["Thread"]);
    expect(promptContextMentionOptions(capabilities, 0, "zz")).toEqual([]);
  });

  it("keeps an available kind visible at the cap, carrying the reason", () => {
    const full = promptContextMentionOptions(capabilities, 4, "", 4);
    expect(full.map(row => row.kind)).toEqual(["file", "folder", "diff"]);
    expect(full.every(row => row.disabled)).toBe(true);
    expect(full[0].disabledReason)
      .toBe("Context limit reached (4). Remove an item to add another.");
  });
});

describe("@ — the menu the composer shows, and the composer's own wiring", () => {
  const menu = (activeIndex: number): string => renderToStaticMarkup(
    createElement(PromptContextMentionMenu, {
      id: "m",
      options: promptContextMentionOptions(
        { file: { available: true }, folder: { available: true } }, 0, ""),
      activeIndex,
      onChoose: () => undefined,
      onActiveIndexChange: () => undefined,
    }));

  it("is a listbox the textarea keeps focus over, not a second focus trap", () => {
    const body = bodyOf(menu(1));
    expect(body).toContain('role="listbox"');
    expect(count(body, 'role="option"')).toBe(2);
    // Every row is out of the tab order: the caret stays in the draft.
    expect(count(body, 'tabindex="-1"')).toBe(2);
    expect(body).toContain('id="m-option-1"');
    expect(count(body, 'aria-selected="true"')).toBe(1);
    expect(body).toContain("Add context");
    expect(body).toContain("↑↓ choose · Enter attach");
  });

  it("says why a row cannot be taken, on the row, instead of its usual blurb", () => {
    const capped = renderToStaticMarkup(createElement(PromptContextMentionMenu, {
      id: "m",
      options: promptContextMentionOptions({ file: { available: true } }, 2, "", 2),
      activeIndex: 0,
      onChoose: () => undefined,
      onActiveIndexChange: () => undefined,
    }));
    expect(bodyOf(capped))
      .toContain("Context limit reached (2). Remove an item to add another.");
    expect(bodyOf(capped)).not.toContain("Attach one file to this prompt.");
    expect(bodyOf(capped)).toContain('aria-disabled="true"');
  });

  it("wears the same option row as the button menu rather than a second style", () => {
    expect(bodyOf(menu(0))).toContain('class="prompt-context-option is-active"');
    expect(PROMPT_CONTEXT_PICKER_CSS)
      .toContain(".prompt-context-mention .prompt-context-option.is-active");
    expect(PROMPT_CONTEXT_PICKER_CSS).toContain("background:var(--row-selected)");
  });

  /** The prompt's own opening tag — the pickers beside it are comboboxes too. */
  const promptTag = (markup: string): string =>
    /<textarea[^>]*>/.exec(bodyOf(markup))?.[0] ?? "";

  it("advertises the completion on the prompt only where @ can finish", () => {
    const wired = card({
      contextCapabilities: { file: { available: true } },
      contextRequests: true,
    });
    expect(promptTag(wired)).toContain('aria-haspopup="listbox"');
    expect(promptTag(wired)).toContain('aria-autocomplete="list"');
    expect(promptTag(wired)).toContain('aria-expanded="false"');
    expect(bodyOf(wired)).toContain('title="Add prompt context — or type @ in the prompt"');

    // No capability and no surface to resolve one: the prompt claims no
    // completion, and the trigger promises no shortcut that does nothing.
    const bare = card();
    expect(promptTag(bare)).not.toContain("aria-haspopup");
    expect(promptTag(bare)).not.toContain("aria-autocomplete");
    expect(promptTag(bare)).not.toContain("aria-expanded");
    expect(bodyOf(bare)).not.toContain("type @ in the prompt");
  });

  it("routes @ keys through the one listbox key contract this textarea already has", () => {
    // Arrows/Enter/Tab/Escape, IME-safe, modifiers left to text editing — the
    // provider-command menu's mapper, so both completions cannot drift apart.
    // It lives in the controller now, which is the unit both prompt surfaces
    // wire, so that is where the contract has to hold.
    expect(promptContextMentionKeyAction.toString())
      .toContain("providerCommandMenuKeyAction");
    const source = Composer.toString();
    expect(source).toContain("promptContextMentionKeyAction");
    expect(/chooseMention/.test(source)).toBe(true);
  });
});

/* ---- the controller both prompt surfaces wire ----------------------------
   The launch card is one of two prompt surfaces; the per-session composer is
   the high-frequency one. These rules are pinned on the controller rather than
   on this card's markup, because the controller is the only thing that can make
   the two agree — a rule proved through the launch card alone would be a rule
   the session composer is free to contradict. */

const MENTION_CAPABILITIES: PromptContextCapabilities = {
  file: { available: true },
  folder: { available: true },
};

/** The controller's view of a draft whose caret/selection is `start`..`end`. */
const viewOf = (draft: string, start: number, end = start, over: {
  selectedCount?: number;
  limit?: number;
  enabled?: boolean;
  suppressed?: boolean;
} = {}): PromptContextMentionView => promptContextMentionView({
  draft,
  state: trackedPromptContextMention(IDLE_PROMPT_CONTEXT_MENTION_STATE,
    { value: draft, selectionStart: start, selectionEnd: end }),
  capabilities: MENTION_CAPABILITIES,
  selectedCount: over.selectedCount ?? 0,
  enabled: over.enabled ?? true,
  ...(over.limit === undefined ? {} : { limit: over.limit }),
  ...(over.suppressed === undefined ? {} : { suppressed: over.suppressed }),
});

describe("@ — the caret the trigger follows", () => {
  it("follows a collapsed caret, but only while the draft holds an @", () => {
    expect(promptContextCaret({ value: "use @f", selectionStart: 6, selectionEnd: 6 })).toBe(6);
    expect(promptContextCaret({ value: "plain prose", selectionStart: 5, selectionEnd: 5 }))
      .toBeNull();
    expect(promptContextCaret({ value: "@f", selectionStart: null, selectionEnd: null }))
      .toBeNull();
  });

  /* The defect this exists for: `selectionStart` is the selection ANCHOR, not a
     caret. Drafting "use @f and more" and drag-selecting " and more" anchors at
     offset 6 — one past the token — which passed every guard, opened the menu,
     and turned the Enter meant to replace the selection into a deletion of "@f".
     A range selection is never a caret, in either drag direction. */
  it("refuses a range selection, which is an anchor and not a caret", () => {
    expect(promptContextCaret({ value: "use @f and more", selectionStart: 6, selectionEnd: 15 }))
      .toBeNull();
    expect(promptContextCaret({ value: "use @f and more", selectionStart: 15, selectionEnd: 6 }))
      .toBeNull();
  });

  it("opens nothing on the text a drag-select just covered, and keeps Enter", () => {
    const draft = "use @f and more";
    expect(viewOf(draft, 6).open).toBe(true);          // the same caret, collapsed
    const dragged = viewOf(draft, 6, 15);
    expect(dragged.open).toBe(false);
    // So Enter is not the menu's: it reaches the surface's own submit, and the
    // user's selected text is still there to be replaced.
    expect(promptContextMentionKeyAction(dragged, draft, { key: "Enter" })).toBeNull();
  });
});

describe("@ — at the item cap the menu explains itself and blocks nothing", () => {
  const draft = "@";
  const capped = (): PromptContextMentionView =>
    viewOf(draft, 1, 1, { selectedCount: 2, limit: 2 });

  it("stays open, carrying the reason on every row", () => {
    const view = capped();
    expect(view.open).toBe(true);
    expect(view.options.every(option => option.disabled === true)).toBe(true);
    expect(view.options[0].disabledReason)
      .toBe("Context limit reached (2). Remove an item to add another.");
  });

  /* The dead end: both keys were consumed unconditionally, so at the cap the
     prompt could neither submit nor be tabbed out of, with no feedback at all.
     A literal "@" is valid prompt text; the submit it swallowed was legitimate. */
  it("leaves Enter to the submit and Tab to the focus order", () => {
    expect(promptContextMentionKeyAction(capped(), draft, { key: "Enter" })).toBeNull();
    expect(promptContextMentionKeyAction(capped(), draft, { key: "Tab" })).toBeNull();
  });

  it("still exits on Escape, which is never the key that gets swallowed", () => {
    expect(promptContextMentionKeyAction(capped(), draft, { key: "Escape" }))
      .toEqual({ type: "dismiss", start: 0 });
  });

  it("removes nothing from the draft when a capped row is taken by any route", () => {
    expect(promptContextMentionChoice(capped(), capped().options[0], draft)).toBeNull();
  });

  it("prints a hint that is true in the state it is printed in", () => {
    expect(promptContextMentionHint(capped().options)).toBe("Esc dismiss");
    expect(promptContextMentionHint(viewOf(draft, 1).options))
      .toBe("↑↓ choose · Enter attach");
    const markup = bodyOf(renderToStaticMarkup(createElement(PromptContextMentionMenu, {
      id: "m", options: capped().options, activeIndex: 0,
      onChoose: () => undefined, onActiveIndexChange: () => undefined,
    })));
    expect(markup).toContain("Esc dismiss");
    expect(markup).not.toContain("Enter attach");
  });
});

describe("@ — only one completion is ever open under this caret", () => {
  it("stays shut while the provider-command menu owns the draft", () => {
    const draft = "/review @f";
    expect(viewOf(draft, draft.length).open).toBe(true);
    expect(viewOf(draft, draft.length, draft.length, { suppressed: true }).open).toBe(false);
  });

  it("stays inert where a chosen kind has no surface that could resolve it", () => {
    const view = viewOf("@f", 2, 2, { enabled: false });
    expect(view.capable).toBe(false);
    expect(view.open).toBe(false);
    expect(promptContextMentionKeyAction(view, "@f", { key: "Enter" })).toBeNull();
  });

  it("takes the row, and takes exactly the typed trigger with it", () => {
    const draft = "review @f";
    const view = viewOf(draft, draft.length);
    expect(view.options).toHaveLength(2);
    expect(promptContextMentionKeyAction(view, draft, { key: "Enter" })).toEqual({
      type: "choose", option: view.options[0],
      edit: { prompt: "review ", caret: 7 },
    });
    expect(promptContextMentionKeyAction(view, draft, { key: "ArrowDown" }))
      .toEqual({ type: "move", index: 1 });
  });
});

describe("@ — the launch card wires the controller, on every path", () => {
  const source = Composer.toString();

  /* A selection change is the other way a caret reaches an "@" — click into the
     middle of a draft, or arrow across one — and it is the path the range-select
     defect above travels. Without this handler the menu only ever opens while
     typing, and both facts above stop describing the running card. */
  it("follows the caret on selection changes, not only on typing", () => {
    expect(/onSelect:\s*\([^)]*\)\s*=>\s*\{\s*trackPromptCaret\(/.test(source)).toBe(true);
    expect(/onChange:[\s\S]{0,400}trackPromptCaret\([^)]*,\s*true\)/.test(source)).toBe(true);
  });

  it("consumes a key only when the controller claims it", () => {
    expect(/mentionKey !== null\)\s*\{[\s\S]{0,80}preventDefault/.test(source)).toBe(true);
  });

  it("drops the caret on blur without remembering a dismissal", () => {
    expect(source).toContain("releasedPromptContextMention");
  });
});

describe("M6 — the launch toolbar is on the radius ramp", () => {
  it("carries no off-token radius literal", () => {
    const css = composerCss;
    expect(css).not.toMatch(/border-radius:\s*\d+px/);
    expect(css).toContain("border-radius: var(--radius-control)");
  });
});

/* M5's floor is 11px, and the "@" menu's head is exactly the case it names:
   uppercase, letter-spaced, informative copy. It shipped at 10px. */
describe("M5 — the @ menu's own head is on the type ramp", () => {
  it("carries no label below the declared legibility floor", () => {
    expect(PROMPT_CONTEXT_PICKER_CSS).not.toMatch(/font-size:\s*10px/);
    expect(PROMPT_CONTEXT_PICKER_CSS).toContain(".prompt-context-mention-head");
  });
});

/* One card, three surfaces. The `kind` prop is the whole difference: a chat has
   no folder, so every control that only means something against one is ABSENT
   rather than disabled, and the copy names what this surface starts. */
describe("composer kinds", () => {
  /* The wordmark is the app's on all three; the line under it is the surface's.
     Code's names a file path and a traceback — a promise a tool-less chat cannot
     keep, and an understatement of what an agent does in a folder. */
  it("names what THIS card starts under the one shared wordmark", () => {
    const introOf = (markup: string): string =>
      /<div class="composer-intro">([\s\S]*?)<\/div>/.exec(bodyOf(markup))?.[1] ?? "";
    const chat = introOf(card({ kind: "chat" }));
    expect(chat).toContain("Interface");
    expect(chat).toContain("Start a conversation");
    expect(chat).toContain("Explore an idea, ask a question, or work through a problem.");
    expect(chat).not.toContain("traceback");
    const agent = introOf(card({ kind: "agent" }));
    expect(agent).toContain("Interface");
    expect(agent).toContain("Describe the job. The agent works in the folder you choose");
    expect(agent).not.toContain("traceback");
    // Code keeps the line it had.
    expect(introOf(card())).toContain("Drop a file path, a traceback, or a rough idea");
  });

  it("a chat card asks only for a message and a model", () => {
    const body = bodyOf(card({ kind: "chat" }));
    expect(body).not.toContain("aria-label=\"Choose repository\"");
    expect(body).not.toContain("Access mode");
    expect(body).not.toContain("Session options");
    expect(body).toContain("placeholder=\"Send a message\"");
    expect(body).toContain("Start chat");
    // The model picker is the one control a chat keeps.
    expect(body).toContain("aria-label=\"Model: Choose model\"");
  });

  /* The rows themselves live in AppSelect's popover, which exists only while
     that popover is open, so the list the picker is HANDED is the fact static
     markup can hold. */
  it("a chat card offers only the engines that can run tool-less", () => {
    expect(composerKindEngines("chat", ENGINES, choicesFor)).toEqual([{ id: "claude" }]);
    expect(composerKindEngines("code", ENGINES, choicesFor)).toEqual(ENGINES);
    expect(composerKindEngines("agent", ENGINES, choicesFor)).toEqual(ENGINES);
    // No chat-capable engine is registered: unsupported models cannot launch.
    expect(composerKindEngines("chat", [{ id: "codex" }], choicesFor))
      .toEqual([]);
  });

  it("an agent card keeps the folder and access controls and starts a task", () => {
    const body = bodyOf(card({ kind: "agent" }));
    expect(body).toContain("aria-label=\"Choose repository\"");
    expect(body).toContain("Access mode");
    expect(body).toContain("placeholder=\"What should the agent do in this folder?\"");
    expect(body).toContain("Start task");
  });

  it("a teammate's task card has no access mode or session options: its own mode decides (review C2)", () => {
    const body = bodyOf(card({ kind: "agent", teammate: "Research" }));
    expect(body).toContain("aria-label=\"Choose repository\"");
    expect(body).toContain("Start task");
    expect(body).not.toContain("Access mode");
    expect(body).not.toContain("Session options");
  });

  /* Same technique as the "@" wiring block above: a node test cannot click the
     submit, but the launch form this card builds is still a fact in its source.
     A chat sends its kind, an empty access mode AND worktree off — neither
     control is on screen, so neither seeded value may travel behind it. The
     worktree half matters most: a stale form value must not travel behind a
     chat that has no repository to branch at all. */
  it("launches with the surface it is, and no access mode or worktree a chat never showed", () => {
    const source = Composer.toString();
    expect(/prompt: commandResolution\.prompt,\s*kind,/.test(source)).toBe(true);
    // Whitespace-tolerant: the bundler reflows an object literal this wide onto
    // its own lines, and the rule being pinned is the two fields, not the shape.
    expect(/kind === "chat" \? \{\s*permissionMode: "",\s*worktree: false\s*\} : \{\}/
      .test(source)).toBe(true);
  });

  /* The preset store is shared with the Code card, so a chat that wrote into it
     would silently reset the access mode a Code card had saved for the same
     model. There is no Access mode control on a chat card, so there is no choice
     of one behind it either — not on launch, and not on a model change. */
  it("a chat never writes an access mode of its own for a model", () => {
    // An existing preset is what a Code card chose; it comes back untouched.
    expect(composerChatPermissionMode("claude", CHOICES,
      { effort: "default", permissionMode: "plan" })).toBe("plan");
    expect(composerChatPermissionMode("claude", CHOICES,
      { effort: "default", permissionMode: "bypassPermissions" })).toBe("bypassPermissions");
    // No preset yet: the engine's own neutral starting point, never the chat
    // form's seeded value — which is where "default" used to leak in from.
    expect(composerChatPermissionMode("claude", CHOICES, undefined))
      .toBe(startingPermissionMode("claude", CHOICES));
    expect(composerChatPermissionMode("claude", CHOICES,
      { effort: "default", permissionMode: "  " }))
      .toBe(startingPermissionMode("claude", CHOICES));
    // And the card routes through it rather than through the Code rule.
    // Read from the source: the rule now lives in composer-model.ts, and a
    // compiled import no longer spells its bare name.
    expect(readFileSync(new URL("./components/Composer.tsx", import.meta.url), "utf8"))
      .toContain("composerChatPermissionMode(engine, choicesFor(engine)");
  });

  /* A card that DOES show the control still remembers what it showed. */
  it("a code launch keeps the mode the card carried when the engine offers none", () => {
    expect(composerRememberedPermissionMode("", "acceptEdits")).toBe("acceptEdits");
    expect(composerRememberedPermissionMode("", "bypassPermissions")).toBe("bypassPermissions");
    // A mode the card DID show and the user DID choose still wins.
    expect(composerRememberedPermissionMode("plan", "acceptEdits")).toBe("plan");
    // Nothing remembered and nothing carried is the store's own "absent".
    expect(composerRememberedPermissionMode("", "")).toBe("default");
    expect(composerRememberedPermissionMode("  ", " acceptEdits ")).toBe("acceptEdits");
  });

  /* launch.ts stopped requiring a folder for a chat, so this card has to stop
     blaming one. "Prompt is required." filed under "cwd" prints above the dock,
     beside a repository button that is not on a chat card at all, and leaves the
     textarea's aria-invalid unset — the field that IS wrong says nothing. */
  it("blames a field the failing card actually has", () => {
    const chat = { engine: "claude", cwd: "", prompt: "", model: "",
      effort: "", permissionMode: "", kind: "chat" } as const;
    expect(composerErrorField(chat)).toBe("prompt");
    expect(composerErrorField({ ...chat, prompt: "hi" })).toBe("model");
    expect(composerErrorField({ ...chat, prompt: "hi", model: "default" })).toBe("model");
    expect(composerErrorField({ ...chat, prompt: "hi", model: "claude-opus-5" })).toBe("form");
    // A folder is still the first thing a card that has one can be missing.
    expect(composerErrorField({ ...chat, kind: "code" })).toBe("cwd");
    expect(composerErrorField({ ...chat, kind: "agent" })).toBe("cwd");
    // No kind is "code", the same default buildLaunchOptions applies.
    expect(composerErrorField({ engine: "claude", cwd: "", prompt: "", model: "",
      effort: "", permissionMode: "" })).toBe("cwd");
  });

  it("a code card is what the card was before any of this", () => {
    const body = bodyOf(card({}));
    expect(body).toContain("aria-label=\"Choose repository\"");
    expect(body).toContain("Access mode");
    expect(body).toContain("placeholder=\"Give Interface a task\"");
    expect(body).toContain("Start chat");
  });
});
