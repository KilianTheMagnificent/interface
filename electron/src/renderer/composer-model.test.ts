import { describe, expect, it } from "vitest";
import {
  advertisedModels, composerFormForKind, composerFormOnReadyModel, composerModelOptions,
  composerProviderForModelValue, concreteEffortLevels,
  COMPOSER_SCOPE_DEFAULTS, COMPOSER_TEAM_DEFAULTS, decodeComposerModelChoice,
  encodeComposerCustomProvider, encodeComposerModelChoice, preferredEffort,
  type ComposerLaunchForm, type EngineChoices,
} from "./composer-model";

// The launch card's model runs without React or a DOM; these are the rules the
// component-level suites (composer.test.ts, ui-components.test.ts) only reached
// through a rendered card.

const choices = (over: Partial<EngineChoices> = {}): EngineChoices => ({
  models: [], efforts: [], permissionModes: [], ...over,
});

describe("concreteEffortLevels", () => {
  it("drops the provider-default sentinels and repeats, keeping order", () => {
    expect(concreteEffortLevels(["", "low", "default", "high", "low", "max"]))
      .toEqual(["low", "high", "max"]);
    expect(concreteEffortLevels([])).toEqual([]);
  });

  it("is the ladder preferredEffort chooses from", () => {
    expect(preferredEffort(["default", "low", "high"], "default", "high")).toBe("high");
    expect(preferredEffort(["default", "low", "high"], "xhigh", null)).toBe("low");
    expect(preferredEffort(["default"], "high")).toBe("");
  });
});

describe("the combined provider+model picker codec", () => {
  it("round-trips a provider and model, including characters JSON must escape", () => {
    for (const [engine, model] of [["claude", "claude-opus-4-6"], ["acp:grok", "grok \"4\""],
      ["codex", "gpt/6:astra"]]) {
      expect(decodeComposerModelChoice(encodeComposerModelChoice(engine, model)))
        .toEqual({ engine, model });
      expect(composerProviderForModelValue(encodeComposerModelChoice(engine, model)))
        .toBe(engine);
    }
  });

  it("names the provider of a custom-model row, and nothing for any other value", () => {
    expect(composerProviderForModelValue(encodeComposerCustomProvider("acp:qwen"))).toBe("acp:qwen");
    expect(composerProviderForModelValue(encodeComposerCustomProvider("  "))).toBeNull();
    expect(composerProviderForModelValue("claude-opus-4-6")).toBeNull();
    expect(composerProviderForModelValue("interface-custom-provider:{")).toBeNull();
    expect(decodeComposerModelChoice(encodeComposerCustomProvider("claude"))).toBeNull();
    expect(decodeComposerModelChoice('interface-model:["claude",""]')).toBeNull();
    expect(decodeComposerModelChoice("interface-model:not json")).toBeNull();
  });
});

describe("advertisedModels", () => {
  it("prefers the rich rows and drops blank and default sentinels", () => {
    const rich = { value: "opus", displayName: "Opus", supportsEffort: true, effortLevels: ["high"] };
    expect(advertisedModels(choices({
      models: ["ignored"],
      modelOptions: [rich, { ...rich, value: "default" }, { ...rich, value: " " }],
    }))).toEqual([rich]);
  });

  it("builds plain rows from the value list when no rich rows were reported", () => {
    expect(advertisedModels(choices({ models: ["default", "sonnet", ""], modelOptions: [] })))
      .toEqual([{ value: "sonnet", displayName: "sonnet", supportsEffort: false, effortLevels: [] }]);
  });
});

describe("composerFormForKind (the Threads card moving between chat and project)", () => {
  const catalogue: Record<string, EngineChoices> = {
    claude: choices({ chat: true, models: ["sonnet", "opus"],
      permissionModes: ["default", "acceptEdits", "auto", "bypassPermissions"] }),
    codex: choices({ models: ["gpt-6"], permissionModes: ["untrusted", "on-request", "never"] }),
  };
  const choicesFor = (engine: string): EngineChoices => catalogue[engine] ?? choices();
  const engines = [{ id: "claude" }, { id: "codex" }];
  const form = (over: Partial<ComposerLaunchForm> = {}): ComposerLaunchForm => ({
    engine: "claude", cwd: "", prompt: "hi", model: "sonnet", effort: "", permissionMode: "default",
    ...COMPOSER_SCOPE_DEFAULTS, ...COMPOSER_TEAM_DEFAULTS, ...over,
  });

  it("keeps a chat-capable engine when going to No project, and clears the worktree toggle", () => {
    const next = composerFormForKind(form({ worktree: true }), "chat", engines, choicesFor, []);
    expect(next.engine).toBe("claude");
    expect(next.model).toBe("sonnet");
    expect(next.worktree).toBe(false);
  });

  it("moves off an engine that cannot chat, onto its newest remembered chat model", () => {
    const presets = [{ engine: "claude", model: "opus", effort: "default", permissionMode: "acceptEdits" }];
    const next = composerFormForKind(form({ engine: "codex", model: "gpt-6",
      permissionMode: "never" }), "chat", engines, choicesFor, presets);
    expect(next.engine).toBe("claude");
    expect(next.model).toBe("opus");
  });

  it("replaces the chat card's unchosen access mode when a project is picked", () => {
    expect(composerFormForKind(form(), "code", engines, choicesFor, []).permissionMode).toBe("auto");
    const presets = [{ engine: "claude", model: "sonnet", effort: "default", permissionMode: "acceptEdits" }];
    expect(composerFormForKind(form(), "code", engines, choicesFor, presets).permissionMode)
      .toBe("acceptEdits");
  });

  it("leaves a mode the owner picked alone", () => {
    const picked = form({ permissionMode: "acceptEdits" });
    expect(composerFormForKind(picked, "code", engines, choicesFor, [])).toBe(picked);
  });
});

/* Review A5 (2026-10-14): the model menu led with two unavailable providers
   above the one that worked, and a fresh Threads card sat on "Choose model". */
describe("ready providers first", () => {
  const catalogue: Record<string, EngineChoices> = {
    claude: choices({ chat: true, models: ["sonnet", "opus"],
      permissionModes: ["default", "acceptEdits", "auto", "bypassPermissions"] }),
    codex: choices({ chat: true, models: ["gpt-6"], permissionModes: ["untrusted", "on-request", "never"] }),
    grok: choices({ chat: true, models: ["grok-5"] }),
  };
  const choicesFor = (engine: string): EngineChoices => catalogue[engine] ?? choices();
  const form = (over: Partial<ComposerLaunchForm> = {}): ComposerLaunchForm => ({
    engine: "claude", cwd: "", prompt: "", model: "", effort: "", permissionMode: "default",
    ...COMPOSER_SCOPE_DEFAULTS, ...COMPOSER_TEAM_DEFAULTS, ...over,
  });

  it("lists unavailable providers after the ready ones, each in registration order", () => {
    const engines = [{ id: "grok", unavailable: "Grok executable could not be started" },
      { id: "codex", unavailable: "initialize: Codex app-server closed stdout" }, { id: "claude" }];
    const groups = [...new Set(composerModelOptions(engines, choicesFor, "claude", "")
      .map(row => row.group))];
    expect(groups).toEqual(["Claude Code", "Grok · Unavailable", "Codex · Unavailable"]);
  });

  it("opens a fresh card on a ready provider's model, never on Choose model", () => {
    const engines = [{ id: "grok", unavailable: "down" }, { id: "claude" }, { id: "codex" }];
    // No memory: the seeded provider's own first model.
    expect(composerFormOnReadyModel(form(), "chat", engines, choicesFor, []))
      .toMatchObject({ engine: "claude", model: "sonnet" });
    // The most recently used model of a ready provider wins.
    const presets = [
      { engine: "grok", model: "grok-5", effort: "default", permissionMode: "default" },
      { engine: "codex", model: "gpt-6", effort: "default", permissionMode: "default" },
    ];
    expect(composerFormOnReadyModel(form(), "chat", engines, choicesFor, presets))
      .toMatchObject({ engine: "codex", model: "gpt-6" });
    // A seed on a down provider moves to a ready one.
    expect(composerFormOnReadyModel(form({ engine: "grok", model: "grok-5" }), "chat", engines,
      choicesFor, [])).toMatchObject({ engine: "claude", model: "sonnet" });
    // A ready seed with a model is the owner's.
    const chosen = form({ model: "opus" });
    expect(composerFormOnReadyModel(chosen, "chat", engines, choicesFor, presets)).toBe(chosen);
    // Nothing ready: unchanged.
    const none = form();
    expect(composerFormOnReadyModel(none, "chat", [{ id: "claude", unavailable: "down" }],
      choicesFor, [])).toBe(none);
  });
});
