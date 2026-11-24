import { describe, expect, it } from "vitest";
import type { ProviderCommand } from "../shared/engine";
import {
  advertisedProviderCommand, canonicalProviderCommandText, contextCompactCommand,
  providerCommandEnterSubmits, providerCommandIntent, providerCommandMatches,
  providerCommandMenuCommit, providerCommandMenuKeyAction,
} from "./provider-commands";

const commands: ProviderCommand[] = [
  { name: "review", description: "Review changes", aliases: ["check"],
    argumentHint: "<target>" },
  { name: "compact", description: "Compact context" },
];

describe("provider command prompt interaction", () => {
  it("opens only for a leading single-line command-name token", () => {
    expect(providerCommandMatches(commands, "/")).toEqual(commands);
    expect(providerCommandMatches(commands, "  /rev")?.map(row => row.name)).toEqual(["review"]);
    expect(providerCommandMatches(commands, "/context")?.map(row => row.name)).toEqual(["compact"]);
    expect(providerCommandMatches(commands, "/review main")).toBeNull();
    expect(providerCommandMatches(commands, "please /review")).toBeNull();
    expect(providerCommandMatches(commands, "/review\nmore")).toBeNull();
  });

  it("resolves only commands advertised by the current session", () => {
    expect(advertisedProviderCommand(commands, "/review main")?.name).toBe("review");
    expect(advertisedProviderCommand(commands, "/CHECK main")?.name).toBe("review");
    expect(advertisedProviderCommand(commands, "/unknown")).toBeNull();
    expect(advertisedProviderCommand(commands, "/model opus")).toBeNull();
    expect(providerCommandIntent(commands, true, "/model opus")).toEqual({
      kind: "blocked", name: "model", reason: "reserved",
    });
    expect(providerCommandIntent(commands, true, "/removed")).toEqual({
      kind: "blocked", name: "removed", reason: "unavailable",
    });
    expect(providerCommandIntent(commands, true, "/review\nmain")).toEqual({
      kind: "blocked", name: "review", reason: "multiline",
    });
    expect(providerCommandIntent(commands, true, "//review")).toBeNull();
    expect(providerCommandIntent(commands, true, "///compact")).toBeNull();
    expect(providerCommandIntent(commands, false, "/provider-without-a-catalogue"))
      .toBeNull();
    expect(canonicalProviderCommandText(commands[0], "/CHECK   keep  spacing"))
      .toBe("/review   keep  spacing");
  });

  it("offers native compact only for a live unlocked advertised command", () => {
    const alias = [{ name: "compress", description: "Compact", aliases: ["compact"] }];
    expect(contextCompactCommand({ commands: alias, live: true, locked: false,
      hasPromptContext: false, canRun: true })).toBe("/compress");
    expect(contextCompactCommand({ commands: alias, live: true, locked: true,
      hasPromptContext: false, canRun: true })).toBeNull();
    expect(contextCompactCommand({ commands: undefined, live: true, locked: false,
      hasPromptContext: false, canRun: true })).toBeNull();
  });

  it("wraps navigation and accepts without treating IME confirmation as Enter", () => {
    expect(providerCommandMenuKeyAction({ key: "ArrowDown" }, 2, 1))
      .toEqual({ type: "move", index: 0 });
    expect(providerCommandMenuKeyAction({ key: "ArrowUp" }, 2, 0))
      .toEqual({ type: "move", index: 1 });
    expect(providerCommandMenuKeyAction({ key: "Home" }, 2, 1))
      .toEqual({ type: "move", index: 0 });
    expect(providerCommandMenuKeyAction({ key: "End" }, 2, 0))
      .toEqual({ type: "move", index: 1 });
    expect(providerCommandMenuKeyAction({ key: "Enter" }, 2, 0))
      .toEqual({ type: "submit", index: 0 });
    expect(providerCommandMenuKeyAction({ key: "Tab" }, 2, 1))
      .toEqual({ type: "choose", index: 1 });
    expect(providerCommandMenuKeyAction({ key: "Escape" }, 0, 0))
      .toEqual({ type: "close" });
    expect(providerCommandMenuKeyAction({ key: "Enter", isComposing: true }, 2, 0))
      .toBeNull();
    expect(providerCommandMenuKeyAction({ key: "Enter", shiftKey: true }, 2, 0))
      .toBeNull();
    expect(providerCommandMenuKeyAction({ key: "Enter", altKey: true }, 2, 0))
      .toBeNull();
    expect(providerCommandMenuKeyAction({ key: "Tab", shiftKey: true }, 2, 0))
      .toBeNull();
    expect(providerCommandMenuKeyAction({ key: "ArrowDown", metaKey: true }, 2, 0))
      .toBeNull();
  });

  it("sends Enter when the draft is already a complete advertised command", () => {
    expect(providerCommandEnterSubmits(commands, "/review")).toBe(true);
    expect(providerCommandEnterSubmits(commands, "  /CHECK")).toBe(true);
    expect(providerCommandEnterSubmits(commands, "/review main")).toBe(true);
    expect(providerCommandEnterSubmits(commands, "/rev")).toBe(false);
    expect(providerCommandEnterSubmits(commands, "/")).toBe(false);
    expect(providerCommandEnterSubmits(commands, "/removed")).toBe(false);
    expect(providerCommandEnterSubmits(commands, "/model opus")).toBe(false);
    expect(providerCommandEnterSubmits([], "/review")).toBe(false);
  });

  it("turns a highlighted menu row into a runnable slash prompt", () => {
    expect(providerCommandMenuCommit(commands[0], "/")).toBe("/review");
    expect(providerCommandMenuCommit(commands[0], "/rev")).toBe("/review");
    expect(providerCommandMenuCommit(commands[0], "/CHECK   keep  spacing"))
      .toBe("/review   keep  spacing");
  });
});
