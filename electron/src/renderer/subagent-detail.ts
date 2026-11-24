import type { EngineEvent } from "../shared/engine";
import type { ConvItem } from "./conversation-reducer";

/* The subagent detail column's pure reads: off the raw log and off the
   transcript. App owns the column's selection and its focus moves, which act
   on the document. */

/** The full task prompt the parent sent this subagent, off the raw log.
 *
 *  The reducer drops the spawning `tool.call` from the rendered items on
 *  purpose — the cell IS that call — but the event log still holds it, and
 *  nothing truncates `input.prompt` on the way in (only `description` is
 *  capped, at 120 chars, in the engine mappers). So the panel's best content
 *  costs no engine change: read it back by callId. */
export function subagentPrompt(events: readonly EngineEvent[], agentKey: string): string | undefined {
  for (const event of events) {
    if (event.kind !== "tool.call" || event.callId !== agentKey) continue;
    const input = event.input;
    if (typeof input !== "object" || input === null) continue;
    const prompt = (input as { prompt?: unknown }).prompt;
    if (typeof prompt === "string" && prompt.trim() !== "") return prompt;
  }
  return undefined;
}

/** The subagent that spawned this one, at any depth, or undefined at the top.
 *  `findSubagentItem` answers "which agent is this"; the tree line needs the
 *  one above it, which only the walk knows. */
export function subagentParent(
  items: readonly ConvItem[], agentKey: string,
): Extract<ConvItem, { kind: "subagent" }> | undefined {
  for (const item of items) {
    if (item.kind !== "subagent") continue;
    if (item.children.some(child => child.kind === "subagent" && child.id === agentKey)) {
      return item;
    }
    const nested = subagentParent(item.children, agentKey);
    if (nested !== undefined) return nested;
  }
  return undefined;
}
