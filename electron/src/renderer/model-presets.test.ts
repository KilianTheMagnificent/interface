import { describe, expect, it, vi } from "vitest";
import {
  MAX_MODEL_PRESETS,
  MODEL_PRESETS_STORAGE_KEY,
  MODEL_PRESETS_VERSION,
  latestModelPresetFor,
  modelPresetFor,
  normalizeModelPresets,
  parseModelPresets,
  readModelPresets,
  rememberModelPreset,
  serializeModelPresets,
  writeModelPresets,
  type ModelPreset,
} from "./model-presets";

const preset = (over: Partial<ModelPreset> = {}): ModelPreset => ({
  engine: "codex",
  model: "gpt-5.6-sol",
  effort: "ultra",
  permissionMode: "bypassPermissions",
  ...over,
});

describe("per-model launch presets", () => {
  it("remembers effort and bypass independently for the same id across providers", () => {
    const presets = rememberModelPreset([
      preset({ engine: "claude", effort: "high", permissionMode: "acceptEdits" }),
    ], preset());

    expect(modelPresetFor(presets, "codex", "gpt-5.6-sol")).toEqual({
      effort: "ultra",
      permissionMode: "bypassPermissions",
    });
    expect(modelPresetFor(presets, "claude", "gpt-5.6-sol")).toEqual({
      effort: "high",
      permissionMode: "acceptEdits",
    });
  });

  it("replaces and promotes only the selected provider/model pair", () => {
    const original = [
      preset({ model: "other", effort: "low" }),
      preset({ effort: "high", permissionMode: "default" }),
    ];
    const next = rememberModelPreset(original, preset());

    expect(next).toEqual([preset(), preset({ model: "other", effort: "low" })]);
    expect(original[1]).toEqual(preset({ effort: "high", permissionMode: "default" }));
  });

  it("normalizes fields, drops invalid entries, de-duplicates, and stays bounded", () => {
    const raw = [
      preset({ engine: " codex ", model: " gpt-5.6-sol ", effort: " ultra " }),
      preset({ effort: "low" }),
      preset({ model: "default" }),
      preset({ engine: "" }),
      ...Array.from({ length: MAX_MODEL_PRESETS + 20 }, (_, index) =>
        preset({ model: `model-${index}` })),
    ];
    const normalized = normalizeModelPresets(raw);

    expect(normalized).toHaveLength(MAX_MODEL_PRESETS);
    expect(normalized[0]).toEqual(preset());
    expect(normalized.filter(value => value.model === "gpt-5.6-sol")).toHaveLength(1);
    expect(normalized.some(value => value.model === "default")).toBe(false);
  });

  it("defaults corrupt neighboring settings without losing a valid identity", () => {
    expect(normalizeModelPresets([{
      engine: "codex", model: "gpt-5.6-sol", effort: 42, permissionMode: "",
    }])).toEqual([preset({ effort: "default", permissionMode: "default" })]);
  });

  it("uses a versioned envelope and rejects malformed or foreign versions", () => {
    const raw = serializeModelPresets([preset()]);
    expect(JSON.parse(raw)).toEqual({
      version: MODEL_PRESETS_VERSION,
      presets: [preset()],
    });
    expect(parseModelPresets(raw)).toEqual([preset()]);
    expect(parseModelPresets(null)).toEqual([]);
    expect(parseModelPresets("{bad json")).toEqual([]);
    expect(parseModelPresets(JSON.stringify({ version: 99, presets: [preset()] })))
      .toEqual([]);
    expect(parseModelPresets(JSON.stringify([preset()]))).toEqual([]);
  });

  it("reads and writes through a failure-safe injectable storage boundary", () => {
    const getItem = vi.fn(() => serializeModelPresets([preset()]));
    expect(readModelPresets({ getItem })).toEqual([preset()]);
    expect(getItem).toHaveBeenCalledWith(MODEL_PRESETS_STORAGE_KEY);

    const setItem = vi.fn();
    expect(writeModelPresets({ setItem }, [preset()])).toBe(true);
    expect(setItem).toHaveBeenCalledWith(
      MODEL_PRESETS_STORAGE_KEY,
      serializeModelPresets([preset()]),
    );
    expect(readModelPresets({ getItem: () => { throw new Error("blocked"); } })).toEqual([]);
    expect(writeModelPresets({
      setItem: () => { throw new Error("quota"); },
    }, [preset()])).toBe(false);
  });
});

describe("latestModelPresetFor", () => {
  it("returns undefined for an empty list", () => {
    expect(latestModelPresetFor([], "claude")).toBeUndefined();
  });

  it("returns undefined when only another engine is stored", () => {
    expect(latestModelPresetFor([preset()], "claude")).toBeUndefined();
  });

  it("returns the newest matching preset", () => {
    const newest = preset({ engine: "claude", model: "opus" });
    const older = preset({ engine: "claude", model: "sonnet" });
    expect(latestModelPresetFor([newest, older], "claude")).toEqual(newest);
  });

  it("does not return a default model if one were somehow present", () => {
    const fallback = preset({ engine: "claude", model: "sonnet" });
    const invalid = preset({ engine: "claude", model: "default" });
    expect(latestModelPresetFor([invalid, fallback], "claude")).toEqual(fallback);
  });
});
