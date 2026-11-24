/**
 * Settings → Threads → Outside agents (T3 section 11, protocol 65): the
 * words for the level Rust holds and the one-line setup for each tool. Rust
 * decides what each level allows; this module only names it.
 */
import type { OutsideAgents, ThreadSettings } from "../shared/threads";
import type { AppSelectOption } from "./components/AppSelect";

/** The levels, least first, as the select lists them. */
export const OUTSIDE_AGENT_OPTIONS: readonly (AppSelectOption & { value: OutsideAgents })[] = [
  { value: "off", label: "Off" },
  { value: "read", label: "Read only" },
  { value: "full", label: "Can start and message threads" },
];

/** The level Rust holds; absent (an older host, or never chosen) is off. */
export function outsideAgentsLevel(settings: ThreadSettings | null | undefined): OutsideAgents {
  return settings?.outsideAgents ?? "off";
}

/** What the row says the levels do. */
export const OUTSIDE_AGENTS_DESCRIPTION = "Lets a Claude Code or Codex running in any terminal on this Mac "
  + "reach your threads through iface mcp. Read only: it lists and reads them. Can start and message threads: "
  + "it also starts threads in your projects, which ask you before they act, and messages the ones it started.";

/** The one-line setup for each tool, run once in a terminal. */
export const OUTSIDE_AGENT_SETUP: readonly { tool: string; command: string }[] = [
  { tool: "Claude Code", command: "claude mcp add interface -- iface mcp" },
  { tool: "Codex", command: "codex mcp add interface -- iface mcp" },
];
