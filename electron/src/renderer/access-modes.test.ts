import { describe, expect, it } from "vitest";
import {
  accessModeOption, accessModeOptions, accessModeTone, accessModeToneClass,
  availableAccessModes,
} from "./access-modes";
import { sessionStatusMode } from "./session-status";

describe("access mode presentation", () => {
  it("presents provider tokens as clear user choices without changing their values", () => {
    expect(accessModeOptions([
      "default", "acceptEdits", "plan", "bypassPermissions",
      "dontAsk", "untrusted", "on-request", "never", "auto",
    ])).toEqual([
      expect.objectContaining({ value: "default", label: "Provider default" }),
      expect.objectContaining({ value: "acceptEdits", label: "Accept edits" }),
      expect.objectContaining({ value: "plan", label: "Plan only" }),
      // The three no-prompt modes are named on the axis that actually separates
      // them — what happens to an action nobody approved — because they all
      // route to one danger hue and "Bypass permissions" / "Don't ask" /
      // "Never ask" left colour and copy both unable to tell them apart.
      expect.objectContaining({ value: "bypassPermissions", label: "Skip prompts",
        meta: "Confirm" }),
      expect.objectContaining({ value: "dontAsk", label: "Deny unapproved" }),
      expect.objectContaining({ value: "untrusted", label: "Ask for untrusted actions" }),
      expect.objectContaining({ value: "on-request", label: "Ask when needed" }),
      expect.objectContaining({ value: "never", label: "Fail unapproved" }),
      expect.objectContaining({ value: "auto", label: "Auto mode" }),
    ]);
  });

  it("describes no-prompt modes without claiming they remove provider restrictions", () => {
    expect(accessModeOption("bypassPermissions").description)
      .toBe("Skip interactive approval prompts; other configured restrictions can still block actions.");
    expect(accessModeOption("dontAsk").description)
      .toBe("Show no approval prompts; deny actions that are not already allowed.");
    expect(accessModeOption("never").description)
      .toBe("Show no approval prompts; actions blocked by the sandbox fail instead of escalating.");
  });

  it("describes Plan as read-only exploration instead of a no-tool sandbox", () => {
    expect(accessModeOption("plan").description)
      .toBe("Read files and run read-only commands to research, without editing source files.");
  });

  it("humanizes an unknown provider mode while retaining its exact token", () => {
    expect(accessModeOption("reviewOnly_mode")).toEqual({
      value: "reviewOnly_mode",
      label: "Review Only mode",
      description: "Provider-defined access policy.",
      meta: "reviewOnly_mode",
    });
  });
});

describe("access mode tone", () => {
  it("gives each supervision level its own hue", () => {
    expect(accessModeTone("plan")).toBe("plan");
    expect(accessModeTone("acceptEdits")).toBe("accept");
    expect(accessModeTone("auto")).toBe("auto");
  });

  it("puts every mode that stops asking on the danger hue", () => {
    // These three differ in what happens to a disallowed action, not in whether
    // anyone is asked first, and it is the asking that the colour is about.
    expect(accessModeTone("bypassPermissions")).toBe("danger");
    expect(accessModeTone("never")).toBe("danger");
    expect(accessModeTone("dontAsk")).toBe("danger");
  });

  it("leaves supervised and unrecognised modes uncoloured", () => {
    // `untrusted` and `on-request` still prompt, so they are ordinary. The
    // unknown token is the load-bearing case: providers mint these freely, and
    // guessing a risk colour for one is worse than printing none.
    for (const value of ["default", "untrusted", "on-request", "reviewOnly_mode", ""]) {
      expect(accessModeTone(value), value).toBe("neutral");
    }
  });

  it("emits no class at all for a neutral mode", () => {
    expect(accessModeToneClass("plan")).toBe("access-tone-plan");
    expect(accessModeToneClass("acceptEdits")).toBe("access-tone-accept");
    expect(accessModeToneClass("bypassPermissions")).toBe("access-tone-danger");
    expect(accessModeToneClass("auto")).toBe("access-tone-auto");
    // Not "access-tone-neutral": a class with no rule behind it would read in
    // the markup as a tone that paints nothing, which is the opposite of what
    // an unremarkable mode should say.
    expect(accessModeToneClass("default")).toBe("");
    expect(accessModeToneClass("on-request")).toBe("");
  });
});

describe("available access modes", () => {
  it("uses a non-empty provider answer instead of merging in the fallback", () => {
    expect(availableAccessModes(["default", "oldMode"], ["default", "auto", "auto"]))
      .toEqual(["default", "auto"]);
  });

  it("uses the deduplicated fallback until the provider reports a vocabulary", () => {
    expect(availableAccessModes(["default", "plan", "default"], []))
      .toEqual(["default", "plan"]);
    expect(availableAccessModes(["default", "plan"], undefined))
      .toEqual(["default", "plan"]);
  });

  it("preserves provider tokens and does not mutate either input", () => {
    const fallback = ["default"];
    const reported = ["reviewOnly_mode", "never"];
    expect(availableAccessModes(fallback, reported)).toEqual(reported);
    expect(fallback).toEqual(["default"]);
    expect(reported).toEqual(["reviewOnly_mode", "never"]);
  });
});

describe("each provider's own names for its modes", () => {
  it("names Claude Code's modes as Claude Code does, picker and status line", () => {
    const rows = accessModeOptions(
      ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"], "claude");
    expect(rows.map(row => row.label)).toEqual(
      ["Manual", "Accept edits", "Plan", "Auto", "Don't Ask", "Bypass Permissions"]);
    expect(rows.map(row => row.value)).toEqual(
      ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"]);
    expect(sessionStatusMode("default", "claude").label).toBe("manual mode on");
    expect(sessionStatusMode("bypassPermissions", "claude").label).toBe("bypass permissions on");
  });

  it("names Codex's, Grok's and the ACP providers' modes in their own words", () => {
    expect(accessModeOptions(["on-request", "never", "untrusted"], "codex").map(row => row.label))
      .toEqual(["Ask for approval", "Never ask", "Untrusted"]);
    expect(accessModeOption("untrusted", "codex").description)
      .toContain("Codex's own CLI no longer offers this mode");
    expect(accessModeOptions(["default", "auto", "bypassPermissions"], "acp:grok").map(row => row.label))
      .toEqual(["Default", "Auto", "Always approve"]);
    expect(sessionStatusMode("bypassPermissions", "acp:grok").label).toBe("always-approve on");
    expect(accessModeOption("default", "acp:opencode").label).toBe("Build");
    expect(accessModeOption("default", "acp:qwen").label).toBe("As set in Qwen");
    expect(accessModeOption("default", "acp:omp").label).toBe("As set in OMP");
    expect(accessModeOption("default", "acp:gemini").label).toBe("Default");
  });

  it("leaves Muse Code in its own permission profile and says so", () => {
    expect(accessModeOption("default", "acp:muse").label).toBe("As set in Muse Code");
    expect(accessModeOption("default", "acp:muse").description).toContain("Auto-review");
  });

  it("leaves Kimi Code in its own permission mode and names its default", () => {
    expect(accessModeOption("default", "acp:kimi").label).toBe("As set in Kimi Code");
    expect(accessModeOption("default", "acp:kimi").description).toContain("Always Ask");
  });

  it("keeps the generic wording for an unknown provider or no engine, and never changes the token", () => {
    expect(accessModeOption("default").label).toBe("Provider default");
    expect(accessModeOption("acceptEdits", "acp:studio").label).toBe("Accept edits");
    expect(accessModeOption("plan", "claude").value).toBe("plan");
    // The tone is the token's, whoever names it.
    expect(accessModeToneClass("bypassPermissions")).toBe("access-tone-danger");
  });
});
