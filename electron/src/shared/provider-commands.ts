import type { ProviderCommand } from "./engine.js";

const PROVIDER_COMMAND_LIMITS = {
  name: 96,
} as const;

// These controls already have stateful, provider-aware UI in Interface. A
// slash command would create a second source of truth. The lifecycle entries
// are excluded for the same reason: changing a provider session id underneath
// the bridge would strand the live handle under the old SessionKey.
const APP_OWNED_COMMANDS = new Set([
  "model", "models", "set-model",
  "effort", "set-effort", "reasoning-effort",
  "permission", "permissions", "permission-mode", "set-permission",
  "access", "access-mode", "set-access", "sandbox", "always-approve",
  "clear", "new", "reset", "resume", "rewind", "rollback", "fork",
  "consult", "handoff", "peer",
  "rename", "set-title",
]);

const COMMAND_NAME = /^[a-z0-9][a-z0-9._:-]*$/i;

/** A complete single-line slash invocation, ready for provider dispatch. */
export function parseProviderCommandText(text: string):
    { name: string; arguments: string } | null {
  const parsed = parseProviderCommandLike(text);
  if (parsed === null || parsed.multiline || appOwns(parsed.name)) return null;
  return { name: parsed.name, arguments: parsed.arguments };
}

/** Identify a slash-shaped draft before deciding whether Interface may run it. */
export function parseProviderCommandLike(text: string):
    { name: string; arguments: string; multiline: boolean } | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("/")) return null;
  // Exactly one leading slash denotes a provider command. Treat `//...` as
  // ordinary text (for example a pasted source-code comment), never as a
  // control action.
  const match = trimmed.match(/^\/(?!\/)([^\s]+)(?:\s+([\s\S]*))?$/);
  if (match === null) return null;
  const name = commandName(match[1]);
  if (name === null) return null;
  return {
    name,
    arguments: (match[2] ?? "").trim(),
    multiline: trimmed.includes("\n") || trimmed.includes("\r"),
  };
}

export function isAppOwnedProviderCommand(name: string): boolean {
  return appOwns(name);
}

// Fast mode is a per-model capability, not a provider-wide one. Interface only
// offers its toggle where the selected model advertises support, so the
// provider's own /fast entry has to obey the same gate or the palette keeps
// offering a command the model cannot serve.
const FAST_MODE_COMMANDS = new Set(["fast", "fast-mode", "fastmode"]);

export function isFastModeProviderCommand(command: ProviderCommand): boolean {
  return [command.name, ...(command.aliases ?? [])]
    .some(name => FAST_MODE_COMMANDS.has(name.toLowerCase()));
}

/** Query after a leading slash while the user is still choosing a command. */
export function providerCommandQuery(text: string): string | null {
  if (!text.startsWith("/") || text.includes("\n") || text.includes("\r")) return null;
  const query = text.slice(1);
  return /\s/.test(query) ? null : query.toLowerCase();
}

export function matchingProviderCommands(commands: readonly ProviderCommand[],
    text: string): ProviderCommand[] {
  const query = providerCommandQuery(text);
  if (query === null) return [];
  if (query === "") return [...commands];
  const starts: ProviderCommand[] = [];
  const contains: ProviderCommand[] = [];
  for (const command of commands) {
    const names = [command.name, ...(command.aliases ?? [])].map(value => value.toLowerCase());
    if (names.some(value => value.startsWith(query))) starts.push(command);
    else if (names.some(value => value.includes(query))
        || command.description.toLowerCase().includes(query)) contains.push(command);
  }
  return [...starts, ...contains];
}

export function providerCommandDraft(command: ProviderCommand): string {
  return `/${command.name}${command.argumentHint ? " " : ""}`;
}

function commandName(value: unknown): string | null {
  if (typeof value !== "string") return null;
  // A valid command is at most 96 characters; scanning megabytes of whitespace
  // to discover it is invalid lets untrusted provider metadata monopolize the
  // main process for no useful result.
  if (value.length > PROVIDER_COMMAND_LIMITS.name + 8) return null;
  const name = value.replace(/^\/+/, "").trim();
  return name.length > 0 && name.length <= PROVIDER_COMMAND_LIMITS.name
      && COMMAND_NAME.test(name) ? name : null;
}

function appOwns(name: string): boolean {
  return APP_OWNED_COMMANDS.has(name.toLowerCase().replaceAll("_", "-"));
}
