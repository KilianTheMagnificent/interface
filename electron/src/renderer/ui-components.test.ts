import { describe, it, expect } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { readFileSync } from "node:fs";
import { Composer } from "./components/Composer";
import {
  COMPOSER_SCOPE_DEFAULTS, COMPOSER_TEAM_DEFAULTS, composerAccessModeAction,
  composerAccessModes,
  composerCustomModelChoice, composerFormForModelChoice, composerModelOptions, composerRepositoryName,
  composerProviderForModelValue, composerScopeOptions, decodeComposerModelChoice,
  effortsForModel, scopeSummary,
  sessionOptionsSummary,
  type ComposerScope, type EngineChoices,
} from "./composer-model";
import { relativeAge } from "./components/SessionLibrary";
import type { PromptContextItem } from "./prompt-context";

/* The renderer's two big presentational components keep their decisions in
   exported pure functions precisely so they can be tested here: vite.config.ts
   collects src/**\/*.test.ts in a NODE environment, which cannot mount a
   component but imports a .tsx module perfectly well.

   It can do more than import them. react-dom/server renders both to static
   markup in this same node environment — no DOM, no new dependency — so the
   MARKUP and the CSS each component actually ships are testable facts here, not
   only its helper functions. What that still cannot see is anything needing a
   live document: effects never run and refs are never attached. Where a rule
   lives on that side, the test says so and reaches for the source instead.

   Everything below is a rule a review found broken. */

const scope = (over: Partial<ComposerScope> = {}): ComposerScope =>
  ({ ...COMPOSER_SCOPE_DEFAULTS, ...over });

const SHOW = { sources: true, tools: true };

describe("composerScopeOptions", () => {
  it("says nothing to the engine at its defaults", () => {
    expect(composerScopeOptions(scope())).toEqual({});
  });

  it("resolves the three settings presets", () => {
    expect(composerScopeOptions(scope({ settingScope: "project" })).settingSources)
      .toEqual(["project"]);
    // [] is the SDK's isolation mode and is NOT the same as omitting the key.
    expect(composerScopeOptions(scope({ settingScope: "isolated" })).settingSources)
      .toEqual([]);
    expect("settingSources" in composerScopeOptions(scope({ settingScope: "all" })))
      .toBe(false);
  });

  it("trims, drops empties and de-duplicates the deny list", () => {
    expect(composerScopeOptions(scope({ disallowedTools: "Bash, WebFetch , Bash" }))
      .disallowedTools).toEqual(["Bash", "WebFetch"]);
  });

  it("sends no tool key at all for a field holding only separators", () => {
    expect("disallowedTools" in composerScopeOptions(scope({ disallowedTools: ", ," })))
      .toBe(false);
  });

  it("carries the worktree flag only when it is on", () => {
    expect(composerScopeOptions(scope({ worktree: true })).worktree).toBe(true);
    expect("worktree" in composerScopeOptions(scope())).toBe(false);
  });

  /* THE SECURITY REGRESSION. `allowedTools` in the Agent SDK is not a
     restriction — sdk.d.ts: "List of tool names that are auto-allowed without
     prompting for permission... To restrict which tools are available, use the
     `tools` option instead." A field labelled "allow only" wired to it removed
     approval prompts for the tools named in it. Nothing this composer resolves
     may put that key on the wire again. */
  it("never emits allowedTools, whatever the form carries", () => {
    const contaminated = { ...scope({ disallowedTools: "Bash" }), allowedTools: "Bash" };
    const out = composerScopeOptions(contaminated as ComposerScope);
    expect(Object.keys(out)).not.toContain("allowedTools");
    expect(out).toEqual({ disallowedTools: ["Bash"] });
  });
});

describe("scopeSummary", () => {
  it("keeps the ordinary full-setup preset out of the resting toolbar", () => {
    expect(scopeSummary(scope(), false, SHOW)).toBe("");
    expect(scopeSummary(scope({ settingScope: "project" }), false, SHOW))
      .toBe("project only");
    expect(scopeSummary(scope({ settingScope: "isolated" }), false, SHOW))
      .toBe("isolated");
  });

  it("has no summary when the engine offers neither control", () => {
    expect(scopeSummary(scope(), false, { sources: false, tools: false })).toBe("");
  });

  it("leads with the armed danger mode", () => {
    expect(scopeSummary(scope({ worktree: true }), true, SHOW))
      .toBe("approval bypass · worktree");
  });

  /* The pill's promise is that it always prints the CURRENT ANSWER. It used to
     print "tools" for any non-empty text, so a stray comma — which resolves to
     no restriction at all — announced one. */
  it("reports no tool restriction for a field that resolves to none", () => {
    expect(scopeSummary(scope({ disallowedTools: ", ," }), false, SHOW))
      .toBe("");
  });

  it("counts the tools that will actually be sent", () => {
    expect(scopeSummary(scope({ disallowedTools: "Bash" }), false, SHOW))
      .toBe("1 tool blocked");
    expect(scopeSummary(scope({ disallowedTools: "Bash, Read, Bash" }), false, SHOW))
      .toBe("2 tools blocked");
  });

  it("never reports tools for an engine that has no tool control", () => {
    expect(scopeSummary(scope({ disallowedTools: "Bash" }), false,
      { sources: false, tools: false })).toBe("");
  });
});

describe("sessionOptionsSummary", () => {
  it("is empty at ordinary defaults so the trigger can stay quiet", () => {
    expect(sessionOptionsSummary(scope(), false, SHOW)).toBe("");
  });

  it("keeps every changed advanced choice visible while collapsed", () => {
    expect(sessionOptionsSummary(scope({
      settingScope: "project", worktree: true, disallowedTools: "Bash, Read",
    }), false, SHOW)).toBe("project only · worktree · 2 tools blocked");
  });

  it("does not mention effort — that control is on the toolbar", () => {
    expect(sessionOptionsSummary(scope(), false, SHOW)).not.toContain("effort");
  });
});

describe("relativeAge", () => {
  const now = Date.parse("2026-01-02T00:00:00.000Z");
  const ago = (ms: number) => new Date(now - ms).toISOString();

  it("formats each unit at its boundary", () => {
    expect(relativeAge(ago(0), now)).toBe("now");
    expect(relativeAge(ago(9_999), now)).toBe("now");
    expect(relativeAge(ago(10_000), now)).toBe("10s");
    expect(relativeAge(ago(59_999), now)).toBe("59s");
    expect(relativeAge(ago(60_000), now)).toBe("1m");
    expect(relativeAge(ago(59 * 60_000), now)).toBe("59m");
    expect(relativeAge(ago(60 * 60_000), now)).toBe("1h");
    expect(relativeAge(ago(24 * 60 * 60_000), now)).toBe("1d");
  });

  it("clamps a future timestamp instead of printing a negative age", () => {
    expect(relativeAge(new Date(now + 3_000).toISOString(), now)).toBe("now");
  });

  it("returns nothing for a missing or unparseable timestamp", () => {
    expect(relativeAge(undefined, now)).toBe("");
    expect(relativeAge("", now)).toBe("");
    expect(relativeAge("not a date", now)).toBe("");
  });
});

const composerCss = readFileSync(new URL("./components/Composer.css", import.meta.url), "utf8");

/* ---- what the components actually ship ----------------------------------- */


/** The markup WITHOUT the injected stylesheet — otherwise every assertion about
 *  a class name is satisfied by the CSS rule that styles it. */
const bodyOf = (markup: string): string =>
  markup.replace(/<style[^>]*>[\s\S]*?<\/style>/, "");

interface CssRule { selectors: string[]; body: string }

/** Flat rule list, comments stripped, declarations whitespace-normalised. */
const rulesOf = (css: string): CssRule[] =>
  [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .map(m => ({
      selectors: m[1].split(",").map(s => s.trim()).filter(s => s !== ""),
      body: m[2].replace(/\s+/g, " ").trim(),
    }));

/** Every declaration that lands on exactly this selector. */
const declFor = (css: string, selector: string): string => {
  const hits = rulesOf(css).filter(r => r.selectors.includes(selector));
  if (hits.length === 0) throw new Error(`no rule for ${selector}`);
  return hits.map(r => r.body).join(" ");
};

const ENGINE_CHOICES: EngineChoices = {
  models: ["claude-opus-5"],
  efforts: ["default", "low", "high"],
  permissionModes: [
    "default", "acceptEdits", "plan", "dontAsk", "auto", "bypassPermissions",
  ],
  settingSources: ["user", "project", "local"],
  toolDenyList: true,
  dangerousModes: ["bypassPermissions"],
};

describe("launch access modes", () => {
  it("keeps every provider-advertised mode in the selector, including bypass", () => {
    expect(composerAccessModes("default", ENGINE_CHOICES.permissionModes))
      .toEqual(ENGINE_CHOICES.permissionModes);
  });

  it("keeps an inherited current token visible without mutating the provider list", () => {
    const offered = [...ENGINE_CHOICES.permissionModes];
    expect(composerAccessModes("managedPolicy", offered))
      .toEqual(["managedPolicy", ...offered]);
    expect(offered).toEqual(ENGINE_CHOICES.permissionModes);
  });

  it("applies supervised modes directly but routes bypass through confirmation", () => {
    expect(composerAccessModeAction("default", ["bypassPermissions"])).toBe("apply");
    expect(composerAccessModeAction("dontAsk", ["bypassPermissions"])).toBe("apply");
    expect(composerAccessModeAction("bypassPermissions", ["bypassPermissions"]))
      .toBe("confirm");
  });
});

describe("model-specific launch effort", () => {
  const rich: EngineChoices = {
    ...ENGINE_CHOICES,
    modelOptions: [
      { value: "reasoning", displayName: "Reasoning", supportsEffort: true,
        effortLevels: ["medium", "ultra"] },
      { value: "fast", displayName: "Fast", supportsEffort: false,
        effortLevels: [] },
    ],
  };

  it("uses provider-advertised levels for the selected model", () => {
    expect(effortsForModel(rich, "reasoning")).toEqual(["medium", "ultra"]);
  });

  it("hides effort for a listed model that explicitly has no effort axis", () => {
    expect(effortsForModel(rich, "fast")).toEqual([]);
  });

  it("hides effort until a concrete model is chosen", () => {
    expect(effortsForModel(rich, "default")).toEqual([]);
    expect(effortsForModel(rich, "")).toEqual([]);
  });

  it("keeps the engine fallback for custom ids", () => {
    expect(effortsForModel(rich, "private-model")).toEqual(["low", "high"]);
  });

  it("reads a bracketed id as an alias only for Claude", () => {
    expect(effortsForModel(rich, "reasoning[1m]", "claude")).toEqual(["medium", "ultra"]);
    // For any other engine that is a custom id, not a row of this catalogue.
    expect(effortsForModel(rich, "reasoning[1m]", "codex")).toEqual(["low", "high"]);
  });
});

describe("one provider-aware launch model picker", () => {
  const byEngine: Record<string, EngineChoices> = {
    claude: {
      ...ENGINE_CHOICES,
      modelOptions: [
        { value: "default", displayName: "Automatic", supportsEffort: false,
          effortLevels: [] },
        { value: "shared-model", displayName: "Shared", supportsEffort: true,
          effortLevels: ["high"] },
      ],
    },
    codex: {
      models: ["default", "shared-model"], efforts: ["default", "medium"],
      permissionModes: ["untrusted", "on-request", "never"],
      modelOptions: [
        { value: "default", displayName: "Automatic", supportsEffort: false,
          effortLevels: [] },
        { value: "shared-model", displayName: "Shared", supportsEffort: true,
          effortLevels: ["medium"] },
      ],
    },
    "acp:grok": {
      models: ["grok-4.6"], efforts: ["low", "high"],
      permissionModes: ["default", "auto", "bypassPermissions"],
      modelOptions: [{
        value: "grok-4.6", displayName: "Grok 4.6",
        supportsEffort: true, effortLevels: ["low", "high"],
      }],
    },
    "acp:qwen": {
      models: ["mlx-community/Qwen3.8-27B-oQ6"], efforts: [],
      permissionModes: ["default"],
      modelOptions: [{
        value: "mlx-community/Qwen3.8-27B-oQ6", displayName: "Qwen 3.8 27B",
        supportsEffort: false, effortLevels: [],
      }],
    },
  };
  const choicesFor = (engine: string): EngineChoices => byEngine[engine];

  it("shows only concrete models and never collides on a shared id", () => {
    const rows = composerModelOptions([
      { id: "claude" }, { id: "codex" }, { id: "acp:grok" }, { id: "acp:qwen" },
    ], choicesFor, "claude", "default");
    const modelRows = rows.filter(row => row.custom !== true);
    expect(modelRows.map(row => [row.group, row.label, row.triggerLabel])).toEqual([
      ["Claude Code", "Shared", "Claude Code · Shared"],
      ["Codex", "Shared", "Codex · Shared"],
      ["Grok", "Grok 4.6", "Grok · Grok 4.6"],
      ["Qwen", "Qwen 3.8 27B", "Qwen · Qwen 3.8 27B"],
    ]);
    expect(modelRows.some(row => row.label === "Automatic"
      || decodeComposerModelChoice(row.value)?.model === "default")).toBe(false);
    expect(new Set(modelRows.map(row => row.value)).size).toBe(modelRows.length);
    expect(modelRows.map(row => composerProviderForModelValue(row.value)))
      .toEqual(["claude", "codex", "acp:grok", "acp:qwen"]);
    expect(modelRows.filter(row => row.label === "Shared").map(row =>
      decodeComposerModelChoice(row.value))).toEqual([
        { engine: "claude", model: "shared-model" },
        { engine: "codex", model: "shared-model" },
      ]);
  });

  it("does not invent a custom row for a Claude [1m] catalogue alias", () => {
    const rows = composerModelOptions([{ id: "claude" }], () => ({
      models: ["claude-fable-5[1m]"],
      modelOptions: [{
        value: "claude-fable-5[1m]", displayName: "Fable 5",
        supportsEffort: true, effortLevels: ["low"],
      }],
      efforts: [],
      permissionModes: ["default"],
    }), "claude", "claude-fable-5");
    expect(rows.filter(row => row.custom !== true).map(row => row.label)).toEqual(["Fable 5"]);
    expect(rows.some(row => row.label === "claude-fable-5")).toBe(false);
  });

  it("keeps unavailable providers visible and retains a selected custom model", () => {
    const rows = composerModelOptions([
      { id: "claude" }, { id: "codex", unavailable: "CLI is not signed in" },
    ], choicesFor, "codex", "private-codex");
    expect(rows.find(row => row.group === "Codex · Unavailable"
      && row.label === "Shared"))
      .toMatchObject({
      description: "Codex is unavailable: CLI is not signed in",
    });
    expect(rows.find(row => row.triggerLabel === "Codex · private-codex")?.description)
      .toBe("Custom Codex model id.");
  });

  it("offers custom entry inside every provider group without parsing the model text", () => {
    const rows = composerModelOptions([
      { id: "claude" }, { id: "codex" }, { id: "acp:grok" }, { id: "acp:qwen" },
    ], choicesFor, "claude", "default");
    const customRows = rows.filter(row => row.custom === true);
    expect(customRows.map(row => [row.group, row.customLabel])).toEqual([
      ["Claude Code", "Custom Claude Code model ID"],
      ["Codex", "Custom Codex model ID"],
      ["Grok", "Custom Grok model ID"],
      ["Qwen", "Custom Qwen model ID"],
    ]);
    const crafted = 'interface-model:["codex","gpt-x"]';
    expect(composerCustomModelChoice(customRows[0]?.value, crafted, "claude"))
      .toEqual({ engine: "claude", model: crafted });
    expect(customRows.map(row => composerProviderForModelValue(row.value)))
      .toEqual(["claude", "codex", "acp:grok", "acp:qwen"]);
    expect(composerProviderForModelValue("interface-custom-provider:not-json")).toBeNull();
  });

  it("uses a configured ACP display name while keeping its stable id in values", () => {
    const rows = composerModelOptions([
      { id: "acp:local-agent", label: "Local Agent" },
    ], () => ({ models: ["local-model"], efforts: ["default"],
      permissionModes: ["default"] }), "acp:local-agent", "local-model");
    expect(rows[0]).toMatchObject({
      group: "Local Agent",
      triggerLabel: "Local Agent · local-model",
    });
    expect(decodeComposerModelChoice(rows[0]?.value ?? "")).toEqual({
      engine: "acp:local-agent", model: "local-model",
    });
  });

  it("switches provider atomically while preserving launch and worktree state", () => {
    const current = {
      engine: "claude", cwd: "/repo", prompt: "Build it", model: "shared-model",
      effort: "high", permissionMode: "bypassPermissions", settingScope: "isolated" as const,
      disallowedTools: "Bash", worktree: true, ...COMPOSER_TEAM_DEFAULTS,
    };
    const next = composerFormForModelChoice(current,
      { engine: "codex", model: "shared-model" }, byEngine.codex);
    // A provider switch with nothing remembered lands on that engine's own
    // non-blocking mode — Codex calls it `never` — rather than on a default
    // that would stop the first tool call to ask.
    expect(next).toEqual({
      engine: "codex", cwd: "/repo", prompt: "Build it", model: "shared-model",
      effort: "medium", permissionMode: "never", settingScope: "all",
      disallowedTools: "", worktree: true, ...COMPOSER_TEAM_DEFAULTS,
    });
  });

  it("preserves a valid effort when changing models inside one provider", () => {
    const current = {
      engine: "claude", cwd: "/repo", prompt: "Build it", model: "default",
      effort: "high", permissionMode: "acceptEdits", settingScope: "project" as const,
      disallowedTools: "WebFetch", worktree: false, ...COMPOSER_TEAM_DEFAULTS,
    };
    expect(composerFormForModelChoice(current,
      { engine: "claude", model: "shared-model" }, byEngine.claude)).toMatchObject({
        engine: "claude", model: "shared-model", effort: "high",
        permissionMode: "acceptEdits", settingScope: "project",
        disallowedTools: "WebFetch",
      });
  });

  it("restores the last effort and access mode for an exact provider/model pair", () => {
    const current = {
      engine: "claude", cwd: "/repo", prompt: "Build it", model: "shared-model",
      effort: "high", permissionMode: "acceptEdits", settingScope: "project" as const,
      disallowedTools: "WebFetch", worktree: true, ...COMPOSER_TEAM_DEFAULTS,
    };
    const codexChoices: EngineChoices = {
      models: ["gpt-5.6-sol"],
      modelOptions: [{ value: "gpt-5.6-sol", displayName: "GPT-5.6 Sol",
        supportsEffort: true, effortLevels: ["low", "ultra"] }],
      efforts: ["default", "low", "ultra"],
      permissionModes: ["default", "bypassPermissions"],
      dangerousModes: ["bypassPermissions"],
    };

    expect(composerFormForModelChoice(
      current,
      { engine: "codex", model: "gpt-5.6-sol" },
      codexChoices,
      { effort: "ultra", permissionMode: "bypassPermissions" },
    )).toEqual({
      engine: "codex", cwd: "/repo", prompt: "Build it", model: "gpt-5.6-sol",
      effort: "ultra", permissionMode: "bypassPermissions", settingScope: "all",
      disallowedTools: "", worktree: true, ...COMPOSER_TEAM_DEFAULTS,
    });
  });

  it("does not restore effort or access values the target no longer advertises", () => {
    const current = {
      engine: "claude", cwd: "/repo", prompt: "Build it", model: "shared-model",
      effort: "high", permissionMode: "acceptEdits", settingScope: "all" as const,
      disallowedTools: "", worktree: false, ...COMPOSER_TEAM_DEFAULTS,
    };
    expect(composerFormForModelChoice(
      current,
      { engine: "codex", model: "shared-model" },
      byEngine.codex,
      { effort: "ultra", permissionMode: "bypassPermissions" },
    )).toMatchObject({ effort: "medium", permissionMode: "never" });
  });

  it("records model switches and both launch-setting controls", () => {
    const source = readFileSync(new URL("./components/Composer.tsx", import.meta.url), "utf8");
    expect(source).toContain("modelPresetFor(modelPresets, choice.engine, choice.model)");
    expect(source).toMatch(
      /rememberModelSettings\(form\);\s*setForm\(next\);\s*rememberModelSettings\(next\)/,
    );
    expect(source).toContain('setRememberedModelSetting("effort", value)');
    expect(source).toContain('setRememberedModelSetting("permissionMode", value)');
    expect(source).toContain(
      'setRememberedModelSetting("permissionMode", dangerousMode)',
    );
  });
});

const composerMarkup = (permissionMode = "default"): string =>
  renderToStaticMarkup(createElement(Composer, {
    open: true,
    defaults: { engine: "claude", cwd: "/w/repo", prompt: "", model: "",
      effort: "default", permissionMode },
    engines: [{ id: "claude" }, { id: "codex" }],
    choicesFor: () => ENGINE_CHOICES,
    onLaunch: () => undefined,
    onBrowse: () => Promise.resolve(null),
    onClose: () => undefined,
  }));

/* ---- the composer's height budget ---------------------------------------- */

/* Measured in the running app over CDP — the real .pane-right, this file's own
   CSS and markup, viewport heights 360 to 1440. The clamp these tests replace
   lost content in 16 of 72 configurations and lost the SAME amount at every
   viewport height, because it and the pane share the 100vh term. See the note
   above .composer-card. */
describe("the composer fits the pane it cannot scroll", () => {
  const css = composerCss;

  it("does not bound the scope panel against the viewport", () => {
    // The disproved shape: `max-height: calc(100vh - 420px)` on the panel. The
    // viewport cancels, and what is left has to guess the card's own chrome.
    expect(declFor(css, ".composer-scope")).not.toMatch(/max-height/);
  });

  it("lets the card fill the New Chat panel instead of guessing a viewport clamp", () => {
    const card = declFor(css, ".composer-card");
    expect(card).toContain("height: 100%");
    expect(card).toContain("box-sizing: border-box");
    expect(card).toContain("overflow: hidden");
    expect(card).not.toMatch(/max-height:\s*max\(/);
  });

  it("leaves the panel as the only thing that gives", () => {
    // Without min-height:0 on BOTH, each is a flex item whose automatic minimum
    // is its own min-content height, the shrink stops before it reaches the
    // panel, and the card scrolls its whole body instead — measured, the panel
    // stayed at 534.16px while the card hid up to 375px of itself.
    expect(declFor(css, ".composer-body")).toContain("min-height: 0");
    // The dock rests as one prompt line plus its controls (owner, 2026-09-12);
    // the textarea's own 32px floor is what holds it open.
    expect(declFor(css, ".composer-box")).toContain("min-height: 0");
    const panel = declFor(css, ".composer-scope");
    expect(panel).toContain("overflow-y: auto");
    // A floor, because the clamp this replaces had none: at vh 360 it resolved
    // to zero and the panel rendered as a 19px strip with every control in it
    // present in the DOM and invisible.
    expect(panel).toMatch(/min-height:\s*100px/);
    expect(panel).toContain("box-sizing: border-box");
  });
});

/* ---- the armed danger pill ----------------------------------------------- */

describe("an armed danger mode reads as danger, not as emphasis", () => {
  const css = composerCss;

  it("gives both armed pills an edge, and neither of them two", () => {
    // Fill alone is not a durable state signal across themes; the edge must
    // continue to identify the armed bypass state.
    expect(declFor(css, ".composer-pill.composer-danger"))
      .toContain("box-shadow: inset 0 0 0 var(--border-w) var(--danger-fg)");
    // The app-styled access picker already has a real border: red there, while
    // the scope disclosure keeps the inset edge above.
    const picker = declFor(css, ".composer-danger-picker .app-select-trigger");
    expect(picker).toContain("border-color: var(--danger-fg)");
    expect(picker).toContain("background: var(--danger-bg)");
  });

  it("never paints the danger word in the app's selection colour", () => {
    // --fg-strong is ordinary emphasis. The words naming the danger must use
    // the semantic danger tier instead.
    for (const rule of rulesOf(css)) {
      if (rule.selectors.some(s => s.startsWith(".composer-pill.composer-danger"))) {
        expect(rule.body).not.toContain("--fg-strong");
      }
    }
    expect(declFor(css, ".composer-pill.composer-danger .composer-pill-v"))
      .toContain("color: var(--danger-fg)");
    // No component renders a pill key any more; its selector would only
    // pretend the element still exists.
    expect(css).not.toContain("composer-pill-k");
  });

  it("arms the scope control too, not only the access selector", () => {
    const armed = bodyOf(composerMarkup("bypassPermissions"));
    expect(armed).toContain("composer-access-picker");
    expect(armed).toContain("composer-danger-picker");
    expect(armed).toContain("composer-pill composer-pill-btn composer-danger");
    const safe = bodyOf(composerMarkup("default"));
    expect(safe).not.toContain("composer-danger-picker");
    expect(safe).not.toContain("composer-pill-btn composer-danger");
  });

  /** The access picker's OWN root, found by the class only it wears. A bare
   *  toContain over the whole card passes on any element that happens to carry
   *  the string, including the stylesheet the card injects. */
  const accessPickerClass = (markup: string): string => {
    const hit = /class="([^"]*composer-access-picker[^"]*)"/u.exec(markup);
    if (hit === null) throw new Error("no access picker in the launch card");
    return hit[1];
  };

  it("colours the pill value for every mode whose risk the app knows", () => {
    expect(accessPickerClass(composerMarkup("plan"))).toContain("access-tone-plan");
    expect(accessPickerClass(composerMarkup("acceptEdits"))).toContain("access-tone-accept");
    expect(accessPickerClass(composerMarkup("auto"))).toContain("access-tone-auto");
    expect(accessPickerClass(composerMarkup("dontAsk"))).toContain("access-tone-danger");
  });

  it("leaves an ordinary mode uncoloured rather than giving it a quiet tone", () => {
    expect(accessPickerClass(composerMarkup("default"))).not.toContain("access-tone");
  });

  it("keeps an armed danger mode's full repaint outranking the tone tint", () => {
    // Both classes ride the same element when bypass is armed, so which colour
    // wins must not depend on which stylesheet the document loaded last: the
    // armed rule carries two classes and beats .access-tone-danger's one.
    const armed = accessPickerClass(composerMarkup("bypassPermissions"));
    expect(armed).toContain("access-tone-danger");
    expect(armed).toContain("composer-danger-picker");
    expect(declFor(composerCss,
      ".composer-danger-picker.composer-access-picker .app-select-value"))
      .toContain("color: var(--danger-fg)");
  });

  it("warns about skipped prompts without claiming provider restrictions disappear", () => {
    const armed = bodyOf(composerMarkup("bypassPermissions"));
    // The pill names the mode in Claude Code's own words (2026-10-06: the
    // provider's names); the description is what makes the promise.
    expect(armed).toContain("Bypass Permissions");
    expect(armed).toContain("Skips every permission prompt");
    expect(armed).toContain("configured deny rules still apply");
    expect(armed).not.toContain("run every command");
    expect(armed).not.toContain("edit every file");
  });
});

describe("launch pickers stay inside the app chrome", () => {
  it("keeps the everyday model, effort, and access choices visible in app chrome", () => {
    const body = bodyOf(composerMarkup());
    for (const label of ["Model", "Access mode"]) {
      expect(body).toContain(`aria-label="${label}:`);
    }
    expect(body).not.toContain('aria-label="Effort:');
    const withModel = bodyOf(renderToStaticMarkup(createElement(Composer, {
      open: true,
      defaults: { engine: "claude", cwd: "/w/repo", prompt: "",
        model: "claude-opus-5", effort: "high", permissionMode: "default" },
      engines: [{ id: "claude" }],
      choicesFor: () => ENGINE_CHOICES,
      onLaunch: () => undefined,
      onBrowse: () => Promise.resolve(null),
      onClose: () => undefined,
    })));
    expect(withModel).toContain('aria-label="Effort: High"');
    expect(withModel).toContain('aria-haspopup="dialog"');
    expect(withModel).toContain("composer-app-picker composer-effort-picker");
    expect(withModel).not.toContain('role="slider"');
    expect(withModel).not.toContain("Automatic");
    expect(body).toContain(">Session options</span>");
    expect(body).toContain('aria-expanded="false"');
    expect(body).not.toContain('class="composer-scope"');
    expect(body).not.toContain('aria-label="Engine:');
    expect(body).not.toContain("<select");
    expect(body).not.toContain("<datalist");
    expect(body).not.toContain("showPicker");
    // Cursor's compact toolbar prints the selected answer, not a field name
    // before every visible picker value. Accessible names remain on both.
    expect(body).not.toContain('class="app-select-prefix"');
    expect(body).toContain('class="app-select-leading"');
    expect(body).toContain('data-provider-logo="claude"');
  });

  it("hides default setup copy and the ordinary provider-policy explanation", () => {
    const body = bodyOf(composerMarkup());
    expect(body).not.toContain("full setup");
    expect(body).not.toContain('class="composer-note"');
    // The compact trigger tooltip/menu may still explain the selected row; the
    // removed element is the separate always-visible sentence below the box.
    expect(body).toContain("Asks before editing files or running commands.");
    // Access itself remains a first-class visible control, in Claude Code's
    // own word for its default mode.
    expect(body).toContain("Manual");
  });

  it("keeps the two-step bypass controls inside Session options", () => {
    const source = readFileSync(new URL("./components/Composer.tsx", import.meta.url), "utf8");
    const panel = source.slice(source.indexOf("{scopeOpen && ("),
      source.indexOf("{/* Ordinary access policy"));
    expect(panel).not.toContain('label="Effort"');
    expect(panel).not.toContain("Reasoning effort");
    expect(panel).toContain("Enable permission bypass…");
    expect(panel).toContain("Yes, skip approval prompts");
    // The dangerous choice opens the panel and arms its confirmation together,
    // wherever the access picker sits in the source.
    expect(source).toMatch(/setScopeOpen\(true\);\n\s*setConfirmingFullAccess\(true\);/u);
    expect(source).toContain("const [scopeOpen, setScopeOpen] = useState(false);");
    expect(source).not.toMatch(/useEffect\([\s\S]{0,200}setScopeOpen\(true\)/);
  });

  it("requires a concrete model while retaining the custom model path", () => {
    const body = bodyOf(composerMarkup());
    const css = composerCss;
    expect(body).toContain("composer-model-picker");
    expect(body).toContain('aria-label="Model: Choose model"');
    expect(body).toContain('aria-label="Choose a model before starting the chat"');
    expect(body).not.toContain('aria-label="Model: Automatic"');
    expect(body).not.toContain('aria-label="Effort: Automatic"');
    expect(body).not.toContain('aria-label="Effort:');
    const trigger = declFor(css, ".composer-model-picker .app-select-trigger");
    expect(trigger).toContain("min-width: 132px");
    expect(trigger).toContain("background: transparent");
    expect(trigger).toContain("border: 0");
  });

  it("uses a configured ACP provider's initials in the model trigger", () => {
    const markup = renderToStaticMarkup(createElement(Composer, {
      open: true,
      defaults: { engine: "acp:local-agent", cwd: "/w/repo", prompt: "", model: "",
        effort: "default", permissionMode: "default" },
      engines: [{ id: "acp:local-agent", label: "Local Agent" }],
      choicesFor: () => ({ models: ["local-model"], efforts: ["default"],
        permissionModes: ["default"] }),
      onLaunch: () => undefined,
      onBrowse: () => Promise.resolve(null),
      onClose: () => undefined,
    }));
    const body = bodyOf(markup);
    expect(body).toContain('data-provider-logo="custom"');
    expect(body).toContain(">LA</span>");
    expect(body).toContain('aria-label="Model: Choose model"');
  });
});

describe("Cursor Agents composer silhouette", () => {
  const markup = composerMarkup();
  const body = bodyOf(markup);
  const css = composerCss;

  it("keeps the repository as a dock pill, not a labelled form field", () => {
    expect(body).toContain('class="composer-context-button"');
    expect(body).toContain('class="composer-context-name">repo</span>');
    expect(body).not.toContain("This Mac");
    expect(body).not.toContain(">Directory</label>");
    expect(composerRepositoryName("/Users/me/work/Interface/")).toBe("Interface");
    expect(composerRepositoryName("C:\\work\\Interface")).toBe("Interface");
    expect(composerRepositoryName("  ")).toBe("No Repo");
  });

  it("uses a full-panel canvas and a compact dock toolbar", () => {
    expect(declFor(css, ".composer-card")).toContain("width: 100%");
    expect(declFor(css, ".composer-dock")).toContain("flex: none");
    const box = declFor(css, ".composer-box");
    expect(box).toContain("box-sizing: border-box");
    expect(box).toContain("min-height: 0");
    expect(box).toContain("border-radius: var(--radius-composer)");
    expect(declFor(css, ".composer-ta")).toContain("font-size: var(--text-prose)");
    expect(declFor(css, ".composer-controls")).toContain("flex-wrap: wrap");
    expect(body).toContain('class="composer-control-row composer-control-primary"');
    expect(body).toContain('class="composer-control-row composer-control-secondary"');
  });

  it("does not draw a selection box around the compound prompt surface", () => {
    expect(rulesOf(css).some(rule => rule.selectors.includes(".composer-box:focus-within")))
      .toBe(false);
  });

  it("orients the launch without adding another settings card", () => {
    expect(body).toContain(">Start a coding session</h1>");
    expect(body).toContain("Drop a file path, a traceback, or a rough idea");
    expect(body).not.toContain("keep steering while the agent works");
    expect(body.indexOf("composer-intro")).toBeLessThan(body.indexOf("composer-dock"));
    expect(body.indexOf("composer-dock")).toBeLessThan(body.indexOf("composer-box"));
  });

  it("puts prompt context first and a clearly named launch action last", () => {
    const context = body.indexOf('aria-label="Add prompt context');
    const model = body.indexOf('aria-label="Model:');
    const launch = body.indexOf('class="composer-submit"');
    expect(context).toBeGreaterThanOrEqual(0);
    expect(context).toBeLessThan(model);
    expect(launch).toBeGreaterThan(model);
    expect(body).not.toContain('class="composer-plus"');
    expect(body).toContain('class="composer-submit-label">Start chat</span>');
    expect(declFor(css, ".composer-submit")).toContain("min-width: 28px");
    expect(declFor(css, ".composer-submit")).toContain("border-radius: var(--radius-pill)");
    // The app's primary-action colour, the same one .conv-send takes; the
    // electric blue stays with selection and focus.
    expect(declFor(css, ".composer-submit")).toContain("color: var(--accent-fg)");
    expect(declFor(css, ".composer-submit")).toContain("background: var(--accent)");
    // Nothing repaints the Start fill: the New thread card's Start left its
    // project's hues for the accent with the Code mode look (owner, 2026-10-23).
    const fills = rulesOf(css).filter(rule => /(?:^|;) ?background:/u.test(rule.body)
      && rule.selectors.some(selector => selector.includes(".composer-submit")));
    expect(fills.flatMap(rule => rule.selectors)).toEqual([
      ".composer-submit", ".composer-submit:hover:not(:disabled)", ".composer-submit:disabled",
    ]);
    expect(declFor(css, ".composer-submit-label")).toContain("display: none");
    expect(css).toContain("@container (max-width: 620px)");
  });

  it("keeps selected context on its own bounded row above the model/access controls", () => {
    const contextItems: PromptContextItem[] = [{
      id: "file-app", kind: "file", label: "App.tsx",
      value: `/a/very/long/project/path/${"nested/".repeat(30)}App.tsx`,
    }];
    const markup = renderToStaticMarkup(createElement(Composer, {
      open: true,
      defaults: { engine: "claude", cwd: "/w/repo", prompt: "Review",
        model: "claude-opus-5",
        effort: "default", permissionMode: "default" },
      engines: [{ id: "claude" }],
      choicesFor: () => ENGINE_CHOICES,
      contextItems,
      contextCapabilities: { file: { available: true } },
      contextOpen: true,
      onContextOpenChange: () => undefined,
      onContextRequest: () => undefined,
      onContextRemove: () => undefined,
      onLaunch: () => undefined,
      onBrowse: () => Promise.resolve(null),
      onClose: () => undefined,
    }));
    const contextBody = bodyOf(markup);
    const row = contextBody.indexOf('class="composer-context-items"');
    const controls = contextBody.indexOf('class="composer-controls"');
    expect(row).toBeGreaterThanOrEqual(0);
    expect(row).toBeLessThan(controls);
    expect(contextBody).toContain('aria-label="Attached prompt context"');
    expect(contextBody).toContain('aria-label="Remove File context App.tsx"');
    expect(contextBody).toContain('role="menu" aria-label="Add prompt context"');
    const contextCss = composerCss;
    expect(declFor(contextCss, ".composer-context-items")).toContain("overflow:hidden");
  });

  it("retains repository browsing in the header and forwards normalized context at launch", () => {
    expect(body).toContain('class="composer-context-button"');
    expect(body).toContain('aria-label="Choose repository"');
    const source = readFileSync(new URL("./components/Composer.tsx", import.meta.url), "utf8");
    expect(source).toContain("onContextRequest?.(kind, form.cwd)");
    expect(source).toContain("onLaunch(effective, composerScopeOptions(effective), selectedContextItems)");
    // The card spreads the drop zone it shares with a session's composer;
    // prompt-drop.test.ts drives its paths, images and pass-throughs.
    expect(source).toContain("promptDropZone({\n    enabled: !launching, pathForFile, onDroppedPaths, onPastedImages,");
    expect(source).toContain("{...dropZone}");
    expect(source).toContain("Drop files to attach");
  });

  it("reports the form repository after browse and post-commit reopen reseeding", () => {
    const source = readFileSync(new URL("./components/Composer.tsx", import.meta.url), "utf8");
    expect(source).toContain("onRepositoryChange?: (cwd: string) => void");
    const browse = source.slice(source.indexOf("async function browse()"),
      source.indexOf("function submit(promptOverride"));
    expect(browse).toMatch(/set\("cwd", picked\);\s*onRepositoryChange\?\.\(picked\)/);
    expect(source).toContain(
      "repositoryChangeRef.current?.(defaultRepositoryRef.current)",
    );
    const renderReseed = source.slice(source.indexOf("if (open !== wasOpen)"),
      source.indexOf("if (!open) return null"));
    expect(renderReseed).not.toContain("onRepositoryChange");
  });

  it("gives provider copy distinct rows instead of concatenating effort text", () => {
    expect(declFor(css, ".composer-model-popover .app-select-copy")).toContain("gap: 3px");
    expect(declFor(css, ".composer-model-popover .app-select-option-description"))
      .toContain("line-height: var(--lh-ui)");
    expect(body).not.toContain("Effort: low, high");
  });
});

describe("the multimodel team switch", () => {
  const composerSource = readFileSync(
    new URL("./components/Composer.tsx", import.meta.url), "utf8");
  const base: ComposerScope = { ...COMPOSER_SCOPE_DEFAULTS };
  const show = { sources: true, tools: true };

  it("says a team is on in the collapsed pill, beside whatever else changed", () => {
    expect(sessionOptionsSummary({ ...base, ...COMPOSER_TEAM_DEFAULTS }, false, show))
      .toBe("");
    expect(sessionOptionsSummary(
      { ...base, team: true, teamEngines: ["claude", "codex"] }, false, show))
      // Three: the lead plus the two providers it may spawn from.
      .toBe("team of 3");
    expect(sessionOptionsSummary(
      { ...base, worktree: true, team: true, teamEngines: ["codex"] }, false, show))
      .toBe("team of 2 · worktree");
    // No provider ticked is the host's "unrestricted", and the pill says that
    // rather than implying a team of one.
    expect(sessionOptionsSummary({ ...base, team: true, teamEngines: [] }, false, show))
      .toBe("team, any engine");
  });

  it("keeps the team when the lead's own provider changes", () => {
    const current = {
      engine: "claude", cwd: "/repo", prompt: "Build it", model: "shared-model",
      effort: "high", permissionMode: "acceptEdits", settingScope: "all" as const,
      disallowedTools: "", worktree: false, team: true, teamEngines: ["codex"],
      teamWorkersWrite: true, teamNotes: "",
    };
    const choices: EngineChoices = {
      models: ["gpt-5.6-sol"], efforts: [], permissionModes: ["never"],
    };
    // Changing which model leads is not a decision about who it may bring in.
    expect(composerFormForModelChoice(current,
      { engine: "codex", model: "gpt-5.6-sol" }, choices))
      .toMatchObject({ team: true, teamEngines: ["codex"] });
  });

  it("is shelved: Session options no longer offers it", () => {
    // 2026-08-27. The form fields, the launch path and the host are intact —
    // only the fieldset was withdrawn, so restoring it is putting it back.
    const panel = composerSource.slice(composerSource.indexOf('id={`${uid}-scope`}'));
    expect(panel).not.toContain("Start this chat as a multimodel team");
    expect(panel).not.toContain("Workers do the work");
    expect(composerSource).not.toContain("Multimodel team");
    expect(COMPOSER_TEAM_DEFAULTS.team).toBe(false);
  });
});
