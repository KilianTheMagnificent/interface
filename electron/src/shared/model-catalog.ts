import type { ModelOption } from "./engine.js";

/**
 * Claude's catalogue tags context-window aliases (`claude-fable-5[1m]`) while
 * a live session reports the bare id (`claude-fable-5`). Those are the same
 * product: a picker must not invent a second raw-id row, and a spawn naming
 * either form must resolve to the same catalogue row.
 *
 * The strip belongs to Claude's rows alone. Applied to every engine it would
 * make `gpt-5.6-luna[anything]` match a real row and reach the provider
 * unchanged, which is the one hole in "an unknown id is refused before any
 * budget is spent"; an unnamed engine keeps the alias-tolerant reading so a
 * caller with no engine in hand still resolves Claude's own two forms.
 */
export function catalogModelKey(value: string, engineId?: string): string {
  const raw = value.trim();
  return engineId === undefined || engineId === "claude"
    ? raw.replace(/\[[^\]]*]$/, "") : raw;
}

export function findCatalogModel<T extends { value: string }>(
    running: string, catalog: readonly T[], engineId?: string): T | undefined {
  const raw = running.trim();
  if (raw === "" || raw === "default") return undefined;
  const exact = catalog.find(model => model.value === raw);
  if (exact !== undefined) return exact;
  const key = catalogModelKey(raw, engineId);
  if (key === "") return undefined;
  return catalog.find(model => catalogModelKey(model.value, engineId) === key);
}

/**
 * The effort ladder one model row actually offers. `ultracode` is Interface's
 * own rung — `xhigh` plus `settings.ultracode` — so it belongs to a Claude row
 * that lists `xhigh` and to no other provider's. Claude's adapter already
 * appends it (`withUltracode`), so the guard here keeps it from doubling.
 */
export function modelEffortLadder(engineId: string, model: ModelOption): string[] {
  const levels = [...model.effortLevels];
  return engineId === "claude" && levels.includes("xhigh") && !levels.includes("ultracode")
    ? [...levels, "ultracode"] : levels;
}
