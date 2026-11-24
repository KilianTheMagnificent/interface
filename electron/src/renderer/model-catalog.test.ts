import { describe, expect, it } from "vitest";
import type { EngineEvent, ModelOption, SessionKey } from "../shared/engine";
import {
  catalogModelKey, findCatalogModel, knownModelOptions, mergeModelOptions,
  modelOptionsFromValues, pickerModelsForRunning,
} from "./model-catalog";

const model = (value: string, over: Partial<ModelOption> = {}): ModelOption => ({
  value,
  displayName: value.toUpperCase(),
  supportsEffort: false,
  effortLevels: [],
  ...over,
});

describe("launch model catalogues", () => {
  it("keeps only concrete models and preserves provider metadata", () => {
    const result = mergeModelOptions(
      [model("gpt-5", { displayName: "GPT-5", description: "Provider copy",
        supportsEffort: true, effortLevels: ["low", "high"] })],
      [model("gpt-5"), model("gpt-5-mini")],
    );
    expect(result).toEqual([
      model("gpt-5", { displayName: "GPT-5", description: "Provider copy",
        supportsEffort: true, effortLevels: ["low", "high"] }),
      model("gpt-5-mini"),
    ]);
  });

  it("normalizes value-only capability fallbacks without inventing features", () => {
    expect(modelOptionsFromValues(["default", "custom-id"])).toEqual([
      { value: "custom-id", displayName: "custom-id", supportsEffort: false,
        effortLevels: [] },
    ]);
  });

  it("uses only the requested provider and the latest catalogue per session", () => {
    const oldList: EngineEvent = { kind: "models.available", models: [model("old")] };
    const newList: EngineEvent = { kind: "models.available", models: [model("new")] };
    const otherList: EngineEvent = { kind: "models.available", models: [model("other")] };
    const events = {
      "codex#one": [oldList, { kind: "notice", message: "x", tone: "info" }, newList],
      "claude#one": [otherList],
    } as Record<SessionKey, EngineEvent[]>;

    expect(knownModelOptions(events, "codex")).toEqual([model("new")]);
  });

  it("drops blank and duplicate ids", () => {
    expect(mergeModelOptions([
      model("  "), model("default"), model("alpha"), model("alpha"),
    ])).toEqual([model("alpha")]);
  });
});

describe("Claude context-window aliases", () => {
  it("treats a bare id and a [1m] catalogue value as the same model", () => {
    expect(catalogModelKey("claude-fable-5[1m]")).toBe("claude-fable-5");
    expect(catalogModelKey("claude-fable-5")).toBe("claude-fable-5");
    const catalog = [
      model("opus[1m]", { displayName: "Opus 5 (1M context)" }),
      model("claude-fable-5[1m]", { displayName: "Fable 5",
        description: "Fable 5 · Most capable for long-running tasks" }),
    ];
    expect(findCatalogModel("claude-fable-5", catalog)?.displayName).toBe("Fable 5");
    expect(findCatalogModel("claude-fable-5[1m]", catalog)?.value).toBe("claude-fable-5[1m]");
  });

  it("labels a running alias with the catalogue name and does not keep both rows", () => {
    const catalog = [
      model("opus[1m]", { displayName: "Opus 5 (1M context)" }),
      model("claude-fable-5[1m]", { displayName: "Fable 5" }),
      model("sonnet", { displayName: "Sonnet 5" }),
    ];
    const rows = pickerModelsForRunning(catalog, "claude-fable-5");
    expect(rows.map(row => [row.value, row.displayName])).toEqual([
      ["claude-fable-5", "Fable 5"],
      ["opus[1m]", "Opus 5 (1M context)"],
      ["sonnet", "Sonnet 5"],
    ]);
  });

  it("strips the bracket alias for Claude's rows only, when the engine is known", () => {
    const catalog = [
      model("claude-fable-5[1m]", { displayName: "Fable 5" }),
      model("gpt-5.6-luna", { displayName: "Luna" }),
    ];
    // Claude's catalogue tags a context-window alias the session reports bare.
    expect(pickerModelsForRunning(catalog, "claude-fable-5", "claude")[0]?.displayName)
      .toBe("Fable 5");
    // No other provider has that alias form, so a bracketed id is a raw id and
    // gets its own row rather than borrowing a catalogued model's label.
    expect(pickerModelsForRunning(catalog, "gpt-5.6-luna[anything]", "codex")[0])
      .toEqual({ value: "gpt-5.6-luna[anything]", displayName: "gpt-5.6-luna[anything]",
        supportsEffort: false, effortLevels: [] });
    // A caller with no engine in hand keeps the alias-tolerant reading.
    expect(pickerModelsForRunning(catalog, "gpt-5.6-luna[anything]")[0]?.displayName)
      .toBe("Luna");
  });

  it("keeps an unknown running id as its own raw row", () => {
    const rows = pickerModelsForRunning([model("sonnet", { displayName: "Sonnet 5" })],
      "claude-mystery");
    expect(rows[0]).toEqual({
      value: "claude-mystery", displayName: "claude-mystery",
      supportsEffort: false, effortLevels: [],
    });
  });
});
