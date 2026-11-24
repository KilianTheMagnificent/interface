import { describe, expect, it } from "vitest";
import {
  isAppOwnedProviderCommand, isFastModeProviderCommand,
  matchingProviderCommands,
  parseProviderCommandLike, parseProviderCommandText,
  providerCommandDraft, providerCommandQuery,
} from "./provider-commands";

describe("provider commands", () => {
  it("parses only complete single-line provider invocations", () => {
    expect(parseProviderCommandText(" /review   main ")).toEqual({
      name: "review", arguments: "main",
    });
    expect(parseProviderCommandText("/model fable")).toBeNull();
    expect(parseProviderCommandText("/review\nthen do this")).toBeNull();
    expect(parseProviderCommandText("//review")).toBeNull();
    expect(parseProviderCommandText("///compact")).toBeNull();
    expect(parseProviderCommandText("ordinary prompt")).toBeNull();
    expect(parseProviderCommandLike("/review\nmain")).toEqual({
      name: "review", arguments: "main", multiline: true,
    });
    expect(isAppOwnedProviderCommand("always_approve")).toBe(true);
    expect(isAppOwnedProviderCommand("set_permission")).toBe(true);
    expect(isAppOwnedProviderCommand("rewind")).toBe(true);
    expect(isAppOwnedProviderCommand("set_title")).toBe(true);
  });

  it("matches the leading command token and keeps arguments out of the menu", () => {
    const commands = [
      { name: "review", description: "Review changes", aliases: ["check"] },
      { name: "compact", description: "Summarize context" },
    ];
    expect(providerCommandQuery("/")).toBe("");
    expect(matchingProviderCommands(commands, "/rev").map(row => row.name)).toEqual(["review"]);
    expect(matchingProviderCommands(commands, "/check").map(row => row.name)).toEqual(["review"]);
    expect(matchingProviderCommands(commands, "/summ").map(row => row.name)).toEqual(["compact"]);
    expect(matchingProviderCommands(commands, "/review main")).toEqual([]);
  });

  it("recognizes the provider's fast-mode command by name or alias", () => {
    expect(isFastModeProviderCommand({ name: "fast", description: "Toggle fast mode" }))
      .toBe(true);
    expect(isFastModeProviderCommand({ name: "speed", description: "Toggle",
      aliases: ["Fast-Mode"] })).toBe(true);
    expect(isFastModeProviderCommand({ name: "compact", description: "Compact" }))
      .toBe(false);
  });

  it("inserts a trailing space only when the provider asks for arguments", () => {
    expect(providerCommandDraft({ name: "compact", description: "Compact" })).toBe("/compact");
    expect(providerCommandDraft({ name: "review", description: "Review",
      argumentHint: "<target>" })).toBe("/review ");
  });
});
