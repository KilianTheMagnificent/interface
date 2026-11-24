import { describe, expect, it } from "vitest";
import type { ModelOption } from "../shared/engine";
import {
  doneLineText,
  formatStatusBarTokens,
  SESSION_STATUS_HINT,
  sessionLine2,
  sessionMeterText,
  sessionStatusBar,
  sessionStatusBarLabel,
  sessionStatusMode,
  sessionStatusModel,
  sessionStatusTokens,
  welcomeLines,
  workingLineText,
} from "./session-status";

const fable: ModelOption = {
  value: "claude-fable-5[1m]",
  displayName: "Fable 5",
  supportsEffort: true,
  effortLevels: ["low", "high"],
};

describe("formatStatusBarTokens", () => {
  it("keeps small counts, then k, then M the way Claude Code writes a 1M window", () => {
    expect(formatStatusBarTokens(0)).toBe("0");
    expect(formatStatusBarTokens(12)).toBe("12");
    expect(formatStatusBarTokens(1500)).toBe("1.5k");
    expect(formatStatusBarTokens(12_000)).toBe("12k");
    expect(formatStatusBarTokens(1_000_000)).toBe("1.0M");
    expect(formatStatusBarTokens(1_200_000)).toBe("1.2M");
    expect(formatStatusBarTokens(10_000_000)).toBe("10M");
    expect(formatStatusBarTokens(-1)).toBe("");
  });
});

describe("sessionStatusTokens", () => {
  it("writes used / size tokens · percent", () => {
    expect(sessionStatusTokens({
      used: 0, size: 1_000_000, percent: 0,
    })).toBe("0 / 1.0M tokens · 0%");
    expect(sessionStatusTokens({
      used: 12_000, size: 200_000, percent: 6,
    })).toBe("12k / 200k tokens · 6%");
  });

  it("says compacting instead of inventing a percentage", () => {
    expect(sessionStatusTokens({ compacting: true, percent: null })).toBe("compacting");
  });

  it("stays quiet when nothing has been measured", () => {
    expect(sessionStatusTokens(undefined)).toBe("");
    expect(sessionStatusTokens({ percent: null })).toBe("");
  });
});

describe("sessionStatusMode", () => {
  it("uses Claude Code's short mode line and its hue", () => {
    expect(sessionStatusMode("auto")).toEqual({
      label: "auto mode on", tone: "auto", mark: "▶▶",
    });
    expect(sessionStatusMode("plan")).toEqual({
      label: "plan mode on", tone: "plan", mark: "○",
    });
    expect(sessionStatusMode("bypassPermissions").tone).toBe("danger");
    expect(sessionStatusMode("").label).toBe("");
  });
});

describe("sessionStatusBar", () => {
  it("resolves a catalog alias to the product name", () => {
    expect(sessionStatusModel("claude-fable-5", [fable])).toBe("Fable 5");
    expect(sessionStatusModel("claude-fable-5", [fable], "Claude", "claude")).toBe("Fable 5");
    // The alias strip is Claude's: for another engine the bare id is a raw id,
    // and the status bar shows it rather than a catalogued model's name.
    expect(sessionStatusModel("claude-fable-5", [fable], "Codex", "codex"))
      .toBe("claude-fable-5");
    expect(sessionStatusBar({
      modelId: "claude-fable-5",
      models: [fable],
      effort: "xhigh",
      permissionMode: "auto",
      usage: { used: 0, size: 1_000_000, percent: 0 },
    })).toEqual({
      model: "Fable 5",
      tokens: "0 / 1.0M tokens · 0%",
      mode: "auto mode on",
      modeTone: "auto",
      modeMark: "▶▶",
      effort: "Extra high",
    });
  });

  it("names the live fields for assistive text and advertises only a real shortcut", () => {
    const view = sessionStatusBar({
      modelId: "claude-fable-5",
      models: [fable],
      effort: "xhigh",
      permissionMode: "auto",
      usage: { used: 0, size: 1_000_000, percent: 0 },
    });
    // The access mode is not in this name: it lives in the pane header now, and
    // announcing it here as well read the same setting twice into one pane.
    expect(sessionStatusBarLabel(view)).toBe(
      "Fable 5, 0 / 1.0M tokens · 0%, Extra high");
    expect(sessionStatusBarLabel(view)).not.toContain("auto mode on");
    expect(SESSION_STATUS_HINT).toBe("⌃⇥ agents");
    expect(SESSION_STATUS_HINT.toLocaleLowerCase()).not.toContain("shift+tab");
  });
});

describe("the two session lines", () => {
  it("writes the meter the way a terminal does — no noun, just the numbers", () => {
    expect(sessionMeterText({ used: 1_200, size: 200_000, percent: 41 }))
      .toBe("1.2k / 200k · 41%");
    expect(sessionMeterText({ compacting: true, percent: null })).toBe("compacting");
    expect(sessionMeterText(undefined)).toBe("");
    expect(sessionMeterText({ percent: null })).toBe("");
  });

  it("rounds a sliver to <1% and prints whichever halves it was given", () => {
    // A percent the provider rounded to 0 while tokens are in flight is a
    // sliver, not an empty window: `0%` beside a used count reads as a bug.
    expect(sessionMeterText({ used: 120, size: 200_000, percent: 0 }))
      .toBe("120 / 200k · <1%");
    expect(sessionMeterText({ used: 1_200, percent: 41 })).toBe("1.2k · 41%");
    // A WINDOW SIZE ALONE IS DROPPED. `200k · 41%` reads as 200k used; the
    // size is the denominator, and a denominator with no numerator beside it
    // says nothing about usage.
    expect(sessionMeterText({ size: 200_000, percent: 41 })).toBe("41%");
    expect(sessionMeterText({ size: 200_000, percent: null })).toBe("");
  });

  it("offers esc only while there is something to interrupt", () => {
    expect(sessionLine2(true)).toBe("esc interrupt · ⌃⇥ agents · / commands");
    expect(sessionLine2(false)).toBe("⌃⇥ agents · / commands");
    // The access mode has left this line for the pane header.
    expect(sessionLine2(true)).not.toContain("mode");
  });
});

describe("welcomeLines", () => {
  it("names the engine, the model and the project, and omits nothing else", () => {
    expect(welcomeLines({
      engineLabel: "Claude Code", version: "2.1.4",
      model: "Fable 5", effort: "xhigh", plan: "max",
      path: "/Users/andre/Desktop/Projects/Interface",
    })).toEqual({
      title: "Claude Code 2.1.4",
      detail: "Fable 5 · Extra high effort · max",
      path: "~/Desktop/Projects/Interface",
    });
  });

  it("takes the number out of a --version line rather than repeating the engine", () => {
    // EngineProbe.version is raw stdout, and the real binaries write it two
    // different ways. Printed whole, each one stutters against the engine name
    // the title already carries: "Claude Code 2.1.251 (Claude Code)".
    expect(welcomeLines({ engineLabel: "Claude Code", version: "2.1.251 (Claude Code)" }).title)
      .toBe("Claude Code 2.1.251");
    expect(welcomeLines({ engineLabel: "Codex", version: "codex-cli 0.150.1" }).title)
      .toBe("Codex 0.150.1");
    // No number in the line is no version, not a fragment of the sentence.
    expect(welcomeLines({ engineLabel: "Codex", version: "unknown" }).title).toBe("Codex");
  });

  it("drops every field the app has not been told, rather than inventing one", () => {
    expect(welcomeLines({ engineLabel: "Codex", model: "gpt-5" })).toEqual({
      title: "Codex",
      detail: "gpt-5",
      path: "",
    });
    // "default" effort is the engine's own choice, not a level worth printing.
    expect(welcomeLines({ engineLabel: "Codex", model: "gpt-5", effort: "default" }).detail)
      .toBe("gpt-5");
    expect(welcomeLines({ engineLabel: "Codex", model: "gpt-5", effort: null }).detail)
      .toBe("gpt-5");
    expect(welcomeLines({}).title).toBe("");
  });
});

describe("workingLineText and doneLineText", () => {
  it("writes Claude Code's busy line", () => {
    expect(workingLineText(134_000)).toBe("Working… (2m 14s)");
    expect(workingLineText(134_000, 1_500)).toBe("Working… (2m 14s · ↓ 1.5k)");
    expect(workingLineText(0)).toBe("Working… (0s)");
  });

  it("says the live activity the caller named, and falls back to Working", () => {
    expect(workingLineText(134_000, undefined, "Thinking")).toBe("Thinking… (2m 14s)");
    expect(workingLineText(134_000, undefined, "  ")).toBe("Working… (2m 14s)");
  });

  it("writes the settled line", () => {
    expect(doneLineText(134_000)).toBe("Turn finished · 2m 14s");
    expect(doneLineText(134_000, 12_000)).toBe("Turn finished · 2m 14s · ↓ 12k");
  });
});
