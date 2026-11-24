import type { CustomizationEntry, CustomizationInventory, CustomizationScope } from "../shared/ipc";

/** Group order is reading order: what defines behavior first, wiring last. */
export const CUSTOMIZATION_GROUPS: readonly { kind: CustomizationEntry["kind"];
  label: string }[] = [
  { kind: "rules", label: "Rules files" },
  { kind: "skill", label: "Skills" },
  { kind: "command", label: "Commands" },
  { kind: "hook", label: "Hooks" },
  { kind: "mcp", label: "MCP servers" },
];

export const CUSTOMIZATION_SCOPE_LABELS: Readonly<Record<CustomizationScope, string>> = {
  project: "Project",
  user: "User",
};

/** One inventory row as the Task details dialog prints it. */
export interface InventoryRow {
  readonly key: string;
  readonly name: string;
  /** Description, matcher or transport; "" when the source gives none. */
  readonly detail: string;
  readonly scope: string;
  readonly sourcePath: string;
  readonly error?: string;
}

/**
 * The customization inventory as readable groups ("Skills · 3", then one row
 * per entry), in `CUSTOMIZATION_GROUPS` order, with empty groups left out.
 * The Task details dialog printed `JSON.stringify` of this until 2026-10-14.
 */
export function inventoryGroups(inventory: CustomizationInventory | undefined):
    readonly { kind: CustomizationEntry["kind"]; label: string; rows: readonly InventoryRow[] }[] {
  const entries = inventory?.entries ?? [];
  return CUSTOMIZATION_GROUPS
    .map(group => ({
      ...group,
      rows: entries.filter(entry => entry.kind === group.kind).map((entry, index) => ({
        key: `${entry.sourcePath}:${entry.name}:${index.toString()}`,
        name: entry.name,
        detail: entry.detail,
        scope: CUSTOMIZATION_SCOPE_LABELS[entry.scope],
        sourcePath: entry.sourcePath,
        ...(entry.error === undefined ? {} : { error: entry.error }),
      })),
    }))
    .filter(group => group.rows.length > 0);
}

/**
 * What a conversation branch's button says: the session's title, the way the
 * sidebar names it. A session no longer listed (archived, forgotten) says so
 * rather than printing its raw `engine:id` key.
 */
export function branchSessionLabel(title: string | undefined): string {
  const named = title?.trim() ?? "";
  return named === "" ? "Session no longer listed" : named;
}
