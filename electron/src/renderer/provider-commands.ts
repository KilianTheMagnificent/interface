import type { ProviderCommand } from "../shared/engine.js";
import {
  isAppOwnedProviderCommand, matchingProviderCommands, parseProviderCommandLike,
  parseProviderCommandText,
} from "../shared/provider-commands.js";

export type ProviderCommandMenuAction =
  | { type: "move"; index: number }
  | { type: "choose"; index: number }
  | { type: "submit"; index: number }
  | { type: "close" };

export interface ProviderCommandKeyEvent {
  key: string;
  shiftKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  isComposing?: boolean;
  nativeEvent?: { isComposing?: boolean; keyCode?: number };
}

export type ProviderCommandIntent =
  | { kind: "command"; command: ProviderCommand }
  | { kind: "blocked"; name: string; reason: "reserved" | "unavailable" | "multiline" };

/** Null means the draft is not currently a command-name query. */
export function providerCommandMatches(commands: readonly ProviderCommand[],
    draft: string): ProviderCommand[] | null {
  // Providers trim the prompt before dispatch. Match that exact leading-space
  // behavior while refusing multiline and mid-sentence slash text.
  const commandText = draft.trimStart();
  if (!commandText.startsWith("/") || commandText.includes("\n")
      || commandText.includes("\r")) return null;
  if (/\s/.test(commandText.slice(1))) return null;
  return matchingProviderCommands(commands, commandText);
}

/** Enter sends when the draft is already a complete advertised command. */
export function providerCommandEnterSubmits(commands: readonly ProviderCommand[],
    draft: string): boolean {
  const parsed = parseProviderCommandLike(draft.trim());
  if (parsed === null || parsed.multiline) return false;
  if (isAppOwnedProviderCommand(parsed.name)) return false;
  return advertisedProviderCommand(commands, draft.trim()) !== null;
}

/** Resolve only a command advertised by this exact live session. */
export function advertisedProviderCommand(commands: readonly ProviderCommand[],
    text: string): ProviderCommand | null {
  const parsed = parseProviderCommandText(text);
  if (parsed === null) return null;
  const wanted = parsed.name.toLowerCase();
  return commands.find(command => command.name.toLowerCase() === wanted
    || command.aliases?.some(alias => alias.toLowerCase() === wanted) === true) ?? null;
}

/** Capability gate for the context meter's native compact action. */
export function contextCompactCommand(input: {
  commands: readonly ProviderCommand[] | undefined;
  live: boolean;
  locked: boolean;
  hasPromptContext: boolean;
  canRun: boolean;
}): string | null {
  if (!input.live || input.locked || input.hasPromptContext || !input.canRun
      || input.commands === undefined) return null;
  const command = advertisedProviderCommand(input.commands, "/compact");
  return command === null ? null : `/${command.name}`;
}

/** Prompt to run when Enter accepts a highlighted command row. */
export function providerCommandMenuCommit(command: ProviderCommand, draft: string): string {
  const parsed = parseProviderCommandLike(draft.trim());
  if (parsed !== null && !parsed.multiline) {
    const wanted = parsed.name.toLowerCase();
    if (command.name.toLowerCase() === wanted
        || command.aliases?.some(alias => alias.toLowerCase() === wanted) === true) {
      return canonicalProviderCommandText(command, draft);
    }
  }
  return `/${command.name}`;
}

/** Replace only the advertised command token; arguments remain byte-for-byte. */
export function canonicalProviderCommandText(command: ProviderCommand, text: string): string {
  const trimmed = text.trim();
  return trimmed.replace(/^\/(?!\/)[^\s]+/, `/${command.name}`);
}

/**
 * Classifies a slash-shaped draft without ever turning a known stale/reserved
 * command into ordinary model input. Unknown catalogues remain permissive so
 * an older provider that cannot advertise commands still accepts manual text.
 */
export function providerCommandIntent(commands: readonly ProviderCommand[],
    catalogueKnown: boolean, text: string): ProviderCommandIntent | null {
  const parsed = parseProviderCommandLike(text);
  if (parsed === null) return null;
  if (parsed.multiline) return { kind: "blocked", name: parsed.name, reason: "multiline" };
  if (isAppOwnedProviderCommand(parsed.name)) {
    return { kind: "blocked", name: parsed.name, reason: "reserved" };
  }
  const wanted = parsed.name.toLowerCase();
  const command = commands.find(row => row.name.toLowerCase() === wanted
    || row.aliases?.some(alias => alias.toLowerCase() === wanted) === true);
  if (command !== undefined) return { kind: "command", command };
  return catalogueKnown
    ? { kind: "blocked", name: parsed.name, reason: "unavailable" }
    : null;
}

export function providerCommandMenuKeyAction(event: ProviderCommandKeyEvent,
    count: number, activeIndex: number): ProviderCommandMenuAction | null {
  if (event.isComposing === true || event.nativeEvent?.isComposing === true
      || event.nativeEvent?.keyCode === 229) return null;
  if (event.key === "Escape") return { type: "close" };
  // Preserve the composer's established text-editing/focus contract:
  // Shift/Alt+Enter insert newlines, Shift+Tab moves backward, and modified
  // arrows remain text selection/navigation rather than menu movement.
  const modified = event.shiftKey === true || event.altKey === true
    || event.metaKey === true || event.ctrlKey === true;
  if (modified) return null;
  if (count <= 0) return null;
  const current = Math.min(Math.max(activeIndex, 0), count - 1);
  if (event.key === "ArrowDown") return { type: "move", index: (current + 1) % count };
  if (event.key === "ArrowUp") return { type: "move", index: (current - 1 + count) % count };
  if (event.key === "Home") return { type: "move", index: 0 };
  if (event.key === "End") return { type: "move", index: count - 1 };
  if (event.key === "Tab") return { type: "choose", index: current };
  if (event.key === "Enter") return { type: "submit", index: current };
  return null;
}
