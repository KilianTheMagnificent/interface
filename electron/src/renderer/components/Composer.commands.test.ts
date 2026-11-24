import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ProviderCommandCatalogue, ProviderCommandScope } from "../../shared/engine";
import { Composer } from "./Composer";
import {
  composerProviderCommandResponseMatches, composerProviderCommandScope,
  composerProviderCommandScopesEqual, resolveComposerCommandPrompt,
  type ComposerLaunchForm, type EngineChoices,
} from "../composer-model";

const form = (over: Partial<ComposerLaunchForm> = {}): ComposerLaunchForm => ({
  engine: "claude",
  cwd: "/work/repo",
  prompt: "",
  model: "claude-opus-5",
  effort: "high",
  permissionMode: "default",
  settingScope: "all",
  disallowedTools: "",
  worktree: false,
  team: false,
  teamEngines: [],
  teamWorkersWrite: true,
  teamNotes: "",
  ...over,
});

const claudeChoices: EngineChoices = {
  models: ["claude-opus-5"], efforts: ["high"], permissionModes: ["default"],
  settingSources: ["user", "project", "local"],
};

const codexChoices: EngineChoices = {
  models: ["gpt-5.6"], efforts: ["high"], permissionModes: ["on-request"],
};

describe("New Chat provider command scope", () => {
  it("keys Claude catalogues on the exact launch settings scope", () => {
    expect(composerProviderCommandScope(form(), claudeChoices)).toEqual({
      engine: "claude", cwd: "/work/repo", model: "claude-opus-5",
    });
    expect(composerProviderCommandScope(
      form({ settingScope: "project" }), claudeChoices)).toEqual({
      engine: "claude", cwd: "/work/repo", model: "claude-opus-5",
      settingSources: ["project"],
    });
    expect(composerProviderCommandScope(
      form({ settingScope: "isolated" }), claudeChoices)).toEqual({
      engine: "claude", cwd: "/work/repo", model: "claude-opus-5",
      settingSources: [],
    });
  });

  it("does not leak a hidden Claude isolation value into Codex or Grok", () => {
    expect(composerProviderCommandScope(form({
      engine: "codex", model: "gpt-5.6", settingScope: "isolated",
    }), codexChoices)).toEqual({
      engine: "codex", cwd: "/work/repo", model: "gpt-5.6",
    });
  });

  it("does not request commands before provider, model, and cwd are concrete", () => {
    expect(composerProviderCommandScope(form({ model: "" }), claudeChoices)).toBeNull();
    expect(composerProviderCommandScope(form({ model: "default" }), claudeChoices)).toBeNull();
    expect(composerProviderCommandScope(form({ cwd: "  " }), claudeChoices)).toBeNull();
  });

  it("invalidates synchronously on any raw request identity change", () => {
    const base = composerProviderCommandScope(form(), claudeChoices);
    expect(composerProviderCommandScopesEqual(base, base)).toBe(true);
    expect(composerProviderCommandScopesEqual(base,
      composerProviderCommandScope(form({ model: "claude-sonnet-5" }), claudeChoices)))
      .toBe(false);
    expect(composerProviderCommandScopesEqual(base,
      composerProviderCommandScope(form({ cwd: "/work/other" }), claudeChoices)))
      .toBe(false);
    expect(composerProviderCommandScopesEqual(base,
      composerProviderCommandScope(form({ settingScope: "isolated" }), claudeChoices)))
      .toBe(false);
  });

  it("uses generation for canonical cwd but requires exact provider/model/settings echoes", () => {
    const requested: ProviderCommandScope = {
      engine: "claude", cwd: "/work/repo/", model: "claude-opus-5",
      settingSources: [],
    };
    const result = (scope: ProviderCommandScope): ProviderCommandCatalogue => ({
      scope, available: true, commands: [],
    });
    expect(composerProviderCommandResponseMatches(result({
      ...requested, cwd: "/work/repo",
    }), requested)).toBe(true);
    expect(composerProviderCommandResponseMatches(result({
      ...requested, engine: "codex",
    }), requested)).toBe(false);
    expect(composerProviderCommandResponseMatches(result({
      ...requested, model: "claude-sonnet-5",
    }), requested)).toBe(false);
    expect(composerProviderCommandResponseMatches(result({
      engine: "claude", cwd: "/work/repo", model: "claude-opus-5",
    }), requested)).toBe(false);
  });
});

describe("New Chat provider command submission", () => {
  const commands = [{
    name: "review", description: "Review changes", aliases: ["check"],
    argumentHint: "<target>",
  }];

  it("canonicalizes only the advertised command token and preserves arguments", () => {
    expect(resolveComposerCommandPrompt(
      commands, true, "/CHECK   branch --deep", "Claude")).toEqual({
      kind: "command",
      command: commands[0],
      prompt: "/review   branch --deep",
    });
  });

  it("blocks stale, reserved, and multiline commands like a live chat", () => {
    expect(resolveComposerCommandPrompt(commands, true, "/removed", "Claude"))
      .toMatchObject({ kind: "blocked" });
    expect(resolveComposerCommandPrompt(commands, false, "/model opus", "Claude"))
      .toMatchObject({ kind: "blocked" });
    expect(resolveComposerCommandPrompt(commands, false, "/review\nthen this", "Claude"))
      .toMatchObject({ kind: "blocked" });
  });

  it("preserves manual provider slash input when discovery is unavailable", () => {
    expect(resolveComposerCommandPrompt([], false, "/provider-native arg", "Provider"))
      .toEqual({ kind: "prompt", prompt: "/provider-native arg" });
  });

  it("does not stall the first New Chat send on an in-flight command catalogue", () => {
    expect(resolveComposerCommandPrompt([], false, "/init", "Claude"))
      .toEqual({ kind: "prompt", prompt: "/init" });
    const source = readFileSync(new URL("./Composer.tsx", import.meta.url), "utf8");
    expect(source).not.toContain("Wait for this model’s commands to finish loading");
    expect(source).not.toContain("Wait for this model's commands to finish loading");
    // A complete command on Enter belongs to the submit, in the menu both
    // prompt surfaces share.
    expect(readFileSync(new URL("../use-provider-command-menu.ts", import.meta.url), "utf8"))
      .toContain("providerCommandEnterSubmits(commands, draft)");
    expect(source).toContain("useProviderCommandMenu({\n    commands: providerCommands, draft: form.prompt,");
  });

  it("does not rebuild the model catalogue from the prompt draft", () => {
    const source = readFileSync(new URL("./Composer.tsx", import.meta.url), "utf8");
    expect(source).toContain("composerModelOptions(");
    expect(source).toContain(
      "[open, kind, engines, form.engine, kindEngines, selectedKindEngine,\n"
      + "    choicesFor, selectedModel, commandChoices]",
    );
    expect(source).not.toContain("advertisedModels(choicesFor(form.engine))");
  });

  it("adds a model-labelled slash affordance without surfacing background state", () => {
    let loads = 0;
    const markup = renderToStaticMarkup(createElement(Composer, {
      open: true,
      defaults: form(),
      engines: [{ id: "claude", label: "Claude" }],
      choicesFor: () => claudeChoices,
      loadProviderCommands: async scope => {
        loads += 1;
        return { scope, available: false, commands: [], reason: "Unavailable" };
      },
      onLaunch: () => undefined,
      onBrowse: () => Promise.resolve(null),
      onClose: () => undefined,
    }));
    expect(markup).toContain('aria-label="Claude Code commands for claude-opus-5"');
    expect(markup).toContain('aria-autocomplete="list"');
    expect(markup).not.toContain("Unavailable");
    // Discovery is an effect, never a render-time side effect.
    expect(loads).toBe(0);
  });

  it("wires race, first-Enter, draft, attachment, and inline-layout invariants", () => {
    const source = readFileSync(new URL("./Composer.tsx", import.meta.url), "utf8");
    expect(source).toContain("generation !== commandRequestGeneration.current");
    expect(source).toContain("composerProviderCommandScopesEqual(commandLoad.requested, commandScope)");
    // The slash menu's keys (shared with a session's composer, and pressed in
    // conversation.test.ts) come before the submit, and a chosen row writes
    // the prompt through `set` and runs through this card's own submit.
    expect(source).toContain("if (commandMenu.onKeyDown(e)) return;");
    expect(source.indexOf("if (commandMenu.onKeyDown(e)) return;"))
      .toBeLessThan(source.indexOf('promptSubmitKeyAction(e) === "submit"'));
    expect(source).toContain('setDraft: text => { set("prompt", text); },');
    expect(source).toContain("submit: text => { submit(text); },");
    expect(source).toContain('placement="inline"');
    expect(source).toContain("pendingCommandLike !== null");
    expect(source).toContain("selectedContextItems.length > 0");
    expect(source).toContain("Provider commands must stay on one line and cannot consume prompt attachments");
    expect(source).toContain("aria-disabled={commandLauncherReason !== null || undefined}");
    expect(source).toContain("commands for ${selectedModel} unavailable. ${commandLauncherReason}");
    expect(source).not.toContain("disabled={launching || form.prompt.trim().length > 0}");
    // One worktree vocabulary (review C2): Local checkout / New worktree.
    expect(source).toContain('<span className="composer-choice-name">New worktree</span>');
    expect(source).toContain("Off, the agent works in the Local checkout.");
    expect(source).not.toContain("Run in an isolated Git worktree");
    expect(source).not.toContain("Run in a throwaway git worktree");
    const loaderEffect = source.slice(source.indexOf("const generation = ++commandRequestGeneration.current"),
      source.indexOf("// Notify only after commit"));
    expect(loaderEffect).not.toContain('set("prompt"');
    expect(loaderEffect).not.toContain("setForm(");
  });
});
