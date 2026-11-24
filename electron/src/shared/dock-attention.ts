/**
 * The Dock badge's second input. The badge counts registry sessions waiting
 * for the owner (main/badge.ts), but a terminal pane's agent asking for input
 * is only known in the renderer (its activity comes from hook frames and
 * output), so the renderer reports how many panes wait that no waiting
 * registry session already stands for. A main-only channel: the Rust host
 * never sees it, and nothing is persisted.
 */

export const DOCK_ATTENTION_CHANNEL = "iface:dockAttention";

export interface DockAttentionV1 {
  readonly version: 1;
  /** Agent panes waiting for input, beyond the waiting registry sessions. */
  readonly waitingPanes: number;
}

const MAX_PANES = 10_000;

/** A renderer report, or null for anything that is not exactly one. */
export function parseDockAttention(value: unknown): DockAttentionV1 | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some(key => key !== "version" && key !== "waitingPanes")) return null;
  const panes = record.waitingPanes;
  if (record.version !== 1 || typeof panes !== "number" || !Number.isInteger(panes)
      || panes < 0 || panes > MAX_PANES) return null;
  return { version: 1, waitingPanes: panes };
}

/** What the Dock icon shows: waiting sessions plus waiting panes. */
export function dockBadgeCount(waitingSessions: number, report: DockAttentionV1 | null): number {
  return waitingSessions + (report?.waitingPanes ?? 0);
}
