import type { EngineEvent, ModelOption, SessionKey } from "../shared/engine";
import { findCatalogModel } from "../shared/model-catalog";

export { catalogModelKey, findCatalogModel, modelEffortLadder } from "../shared/model-catalog";

/**
 * Live-session picker rows. A running alias of a catalogued model reuses that
 * model's label and replaces the alias so the trigger never shows a raw id.
 */
export function pickerModelsForRunning(
    catalog: readonly ModelOption[], running: string, engineId?: string): ModelOption[] {
  const listed = catalog.filter(model => {
    const value = model.value.trim();
    return value !== "" && value !== "default";
  });
  const raw = running.trim();
  if (raw === "" || raw === "default") return listed;
  const match = findCatalogModel(raw, listed, engineId);
  if (match === undefined) {
    return [
      { value: raw, displayName: raw, supportsEffort: false, effortLevels: [] },
      ...listed,
    ];
  }
  if (match.value === raw) return listed;
  return [
    { ...match, value: raw },
    ...listed.filter(model => model.value !== match.value),
  ];
}

/** Turn a capability's value-only fallback into rows the polished picker can draw. */
export function modelOptionsFromValues(values: readonly string[]): ModelOption[] {
  return values
    .map(value => value.trim())
    .filter(value => value !== "" && value !== "default")
    .map(value => ({
        value,
        displayName: value,
        supportsEffort: false,
        effortLevels: [],
      }));
}

/**
 * Merge catalogues without losing provider descriptions or per-model effort.
 * Earlier groups are more authoritative. Provider-default sentinels are not
 * models and never become selectable rows: Interface requires a concrete id.
 */
export function mergeModelOptions(
    ...groups: ReadonlyArray<readonly ModelOption[] | undefined>): ModelOption[] {
  const byValue = new Map<string, ModelOption>();
  for (const group of groups) {
    for (const model of group ?? []) {
      const value = model.value.trim();
      if (value === "" || value === "default" || byValue.has(value)) continue;
      byValue.set(value, { ...model, value });
    }
  }
  return [...byValue.values()];
}

/**
 * Model rows already advertised by this provider's sessions in the current app
 * run. They remain a useful fallback if a later health probe cannot open its
 * control channel, but are never pooled across providers.
 */
export function knownModelOptions(
    events: Record<SessionKey, EngineEvent[]>, engine: string): ModelOption[] {
  const prefix = `${engine}#`;
  const groups: ModelOption[][] = [];
  for (const [key, log] of Object.entries(events)) {
    if (!key.startsWith(prefix)) continue;
    for (let index = log.length - 1; index >= 0; index--) {
      const event = log[index];
      if (event.kind !== "models.available") continue;
      groups.push(event.models);
      break;
    }
  }
  return mergeModelOptions(...groups);
}
