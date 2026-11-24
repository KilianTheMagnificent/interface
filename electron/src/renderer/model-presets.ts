import { asRecord, normalizedToken } from "./helpers";

/** A model's two launch settings that are meaningful across repositories. */
export interface ModelPreset {
  engine: string;
  model: string;
  effort: string;
  permissionMode: string;
}

export type ModelPresetSettings = Pick<ModelPreset, "effort" | "permissionMode">;

export const MODEL_PRESETS_STORAGE_KEY = "interface.modelPresets.v1";
export const MODEL_PRESETS_VERSION = 1;
export const MAX_MODEL_PRESETS = 100;

const MAX_ENGINE_CHARS = 128;
const MAX_MODEL_CHARS = 512;
const MAX_SETTING_CHARS = 128;

interface ModelPresetEnvelope {
  version: typeof MODEL_PRESETS_VERSION;
  presets: ModelPreset[];
}

export interface ModelPresetStorageReader {
  getItem(key: string): string | null;
}

export interface ModelPresetStorageWriter {
  setItem(key: string, value: string): void;
}

function normalizeModelPreset(value: unknown): ModelPreset | null {
  const record = asRecord(value);
  if (record === null) return null;
  const engine = normalizedToken(record.engine, MAX_ENGINE_CHARS);
  const model = normalizedToken(record.model, MAX_MODEL_CHARS);
  if (engine === null || model === null || model === "default") return null;
  return {
    engine,
    model,
    effort: normalizedToken(record.effort, MAX_SETTING_CHARS) ?? "default",
    permissionMode: normalizedToken(record.permissionMode, MAX_SETTING_CHARS) ?? "default",
  };
}

function sameModel(left: Pick<ModelPreset, "engine" | "model">,
    right: Pick<ModelPreset, "engine" | "model">): boolean {
  return left.engine === right.engine && left.model === right.model;
}

/**
 * Normalize newest-first presets without allowing corrupt or unbounded storage
 * to grow renderer state. The provider is part of identity because two engines
 * may advertise the same model value with different settings vocabularies.
 */
export function normalizeModelPresets(value: unknown): ModelPreset[] {
  if (!Array.isArray(value)) return [];
  const presets: ModelPreset[] = [];
  // The same (engine, model) identity `sameModel` compares, as one Set key.
  const seen = new Set<string>();
  for (const raw of value) {
    const preset = normalizeModelPreset(raw);
    if (preset === null) continue;
    const identity = `${preset.engine}\0${preset.model}`;
    if (seen.has(identity)) continue;
    seen.add(identity);
    presets.push(preset);
    if (presets.length === MAX_MODEL_PRESETS) break;
  }
  return presets;
}

/** Tolerant, versioned localStorage decoder. Malformed or foreign data fails closed. */
export function parseModelPresets(raw: string | null | undefined): ModelPreset[] {
  if (raw === null || raw === undefined || raw.trim() === "") return [];
  try {
    const envelope = asRecord(JSON.parse(raw) as unknown);
    if (envelope === null || envelope.version !== MODEL_PRESETS_VERSION) return [];
    return normalizeModelPresets(envelope.presets);
  } catch {
    return [];
  }
}

/** Stable serialization of only the same normalized, bounded shape we accept. */
export function serializeModelPresets(presets: readonly ModelPreset[]): string {
  const envelope: ModelPresetEnvelope = {
    version: MODEL_PRESETS_VERSION,
    presets: normalizeModelPresets(presets),
  };
  return JSON.stringify(envelope);
}

/** Newest stored preset for this engine. Presets are already newest-first. */
export function latestModelPresetFor(
  presets: readonly ModelPreset[], engine: string,
): ModelPreset | undefined {
  const id = engine.trim();
  if (id === "") return undefined;
  return presets.find(preset => preset.engine === id && preset.model !== "default");
}

/** Find one exact provider/model pair without exposing the stored array for mutation. */
export function modelPresetFor(presets: readonly ModelPreset[], engine: string,
    model: string): ModelPresetSettings | undefined {
  const identity = normalizeModelPreset({ engine, model,
    effort: "default", permissionMode: "default" });
  if (identity === null) return undefined;
  const found = presets.find(preset => sameModel(preset, identity));
  return found === undefined
    ? undefined
    : { effort: found.effort, permissionMode: found.permissionMode };
}

/** Move the latest selection to the front, retaining an immutable bounded history. */
export function rememberModelPreset(presets: readonly ModelPreset[],
    preset: ModelPreset): ModelPreset[] {
  const normalized = normalizeModelPreset(preset);
  const current = normalizeModelPresets(presets);
  if (normalized === null) return current;
  return [normalized, ...current.filter(existing => !sameModel(existing, normalized))]
    .slice(0, MAX_MODEL_PRESETS);
}

export function readModelPresets(storage: ModelPresetStorageReader): ModelPreset[] {
  try {
    return parseModelPresets(storage.getItem(MODEL_PRESETS_STORAGE_KEY));
  } catch {
    return [];
  }
}

/** Returns false for blocked/quota-limited storage while in-memory presets remain usable. */
export function writeModelPresets(storage: ModelPresetStorageWriter,
    presets: readonly ModelPreset[]): boolean {
  try {
    storage.setItem(MODEL_PRESETS_STORAGE_KEY, serializeModelPresets(presets));
    return true;
  } catch {
    return false;
  }
}
