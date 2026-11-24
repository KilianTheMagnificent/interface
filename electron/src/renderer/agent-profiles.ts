/**
 * Agent profiles: the reusable "who" behind an Agent-surface task — a name, the
 * provider it runs on, optional model/effort, standing instructions, and an
 * avatar colour. The Rust agent store is canonical. This module is the legacy
 * import decoder for the `interface.agentProfiles.v1` localStorage envelope
 * (one versioned envelope, tolerant normalize, bounded, a read that never
 * throws) that feeds `agentControl({ kind: "importLegacy" })`, plus pure
 * presentation helpers over the store shape.
 *
 * The envelope's assignments (session key -> agent id) date from when the
 * renderer recorded which task an agent launched; the decoder still bounds
 * and filters them so the import receives exactly what was stored.
 */
import { asRecord, normalizedToken } from "./helpers";

export const AGENT_COLORS = ["teal", "coral", "sky", "sand", "violet", "moss"] as const;
export type AgentColor = typeof AGENT_COLORS[number];

export type { AgentProfile } from "../shared/agents";
import type { AgentCommand, AgentProfile } from "../shared/agents";
import { AGENT_SHAPES } from "../shared/agents";

export interface AgentProfileStore {
  agents: AgentProfile[];
  /** Session key -> the id of the agent whose launch created it. */
  assignments: Record<string, string>;
}

export const AGENT_PROFILES_STORAGE_KEY = "interface.agentProfiles.v1";
export const AGENT_PROFILES_VERSION = 1;
export const MAX_AGENT_PROFILES = 50;
export const MAX_AGENT_ASSIGNMENTS = 500;

const MAX_ID_CHARS = 64;
const MAX_NAME_CHARS = 60;
const MAX_ENGINE_CHARS = 128;
const MAX_MODEL_CHARS = 512;
const MAX_EFFORT_CHARS = 128;
const MAX_INSTRUCTIONS_CHARS = 8000;
const MAX_SESSION_KEY_CHARS = 256;

interface AgentProfileEnvelope {
  version: typeof AGENT_PROFILES_VERSION;
  agents: AgentProfile[];
  assignments: Record<string, string>;
}

export interface AgentProfileStorageReader {
  getItem(key: string): string | null;
}

const emptyStore = (): AgentProfileStore => ({ agents: [], assignments: {} });

function normalizeAgentProfile(value: unknown): AgentProfile | null {
  const record = asRecord(value);
  if (record === null) return null;
  const id = normalizedToken(record.id, MAX_ID_CHARS);
  const name = normalizedToken(record.name, MAX_NAME_CHARS);
  const engine = normalizedToken(record.engine, MAX_ENGINE_CHARS);
  // Identity, a person-readable name and a provider to run on are what make a
  // profile usable at all; an entry missing one of them is not repairable here.
  if (id === null || name === null || engine === null) return null;
  const color = AGENT_COLORS.find(candidate => candidate === record.color);
  return {
    id,
    name,
    engine,
    // The optional three are the ones a bad value CAN be repaired for: an
    // over-long or non-string setting simply falls back to "provider default"
    // rather than taking the whole agent down with it.
    model: normalizedToken(record.model, MAX_MODEL_CHARS) ?? "",
    effort: normalizedToken(record.effort, MAX_EFFORT_CHARS) ?? "",
    instructions: normalizedToken(record.instructions, MAX_INSTRUCTIONS_CHARS) ?? "",
    color: color ?? AGENT_COLORS[0],
    ...(AGENT_SHAPES.some(shape => shape === record.shape) ? { shape: record.shape as AgentProfile["shape"] } : {}),
  };
}

/**
 * Normalize the whole store without letting corrupt or unbounded storage grow
 * renderer state. Assignments are filtered against the agents that survived, so
 * a pointer to a deleted agent can never resurrect a header.
 */
export function normalizeAgentProfileStore(value: unknown): AgentProfileStore {
  const record = asRecord(value);
  if (record === null) return emptyStore();
  const agents: AgentProfile[] = [];
  const known = new Set<string>();
  if (Array.isArray(record.agents)) {
    for (const raw of record.agents) {
      const agent = normalizeAgentProfile(raw);
      if (agent === null || known.has(agent.id)) continue;
      known.add(agent.id);
      agents.push(agent);
      if (agents.length === MAX_AGENT_PROFILES) break;
    }
  }
  const kept: [string, string][] = [];
  const assignmentSource = asRecord(record.assignments);
  if (assignmentSource !== null) {
    for (const [key, agentId] of Object.entries(assignmentSource)) {
      const sessionKey = normalizedToken(key, MAX_SESSION_KEY_CHARS);
      const id = normalizedToken(agentId, MAX_ID_CHARS);
      if (sessionKey === null || id === null || !known.has(id)) continue;
      kept.push([sessionKey, id]);
    }
  }
  // Newest last, so the bound drops the oldest launches rather than the ones
  // whose task is most likely still on screen.
  return { agents, assignments: Object.fromEntries(kept.slice(-MAX_AGENT_ASSIGNMENTS)) };
}

/** Tolerant, versioned localStorage decoder. Malformed or foreign data fails closed. */
export function parseAgentProfiles(raw: string | null | undefined): AgentProfileStore {
  if (raw === null || raw === undefined || raw.trim() === "") return emptyStore();
  try {
    const envelope = asRecord(JSON.parse(raw) as unknown);
    if (envelope === null || envelope.version !== AGENT_PROFILES_VERSION) return emptyStore();
    return normalizeAgentProfileStore(envelope);
  } catch {
    return emptyStore();
  }
}

/** Stable serialization of only the same normalized, bounded shape we accept. */
export function serializeAgentProfiles(store: AgentProfileStore): string {
  const normalized = normalizeAgentProfileStore(store);
  const envelope: AgentProfileEnvelope = {
    version: AGENT_PROFILES_VERSION,
    agents: normalized.agents,
    assignments: normalized.assignments,
  };
  return JSON.stringify(envelope);
}

export function readAgentProfiles(storage: AgentProfileStorageReader): AgentProfileStore {
  try {
    return parseAgentProfiles(storage.getItem(AGENT_PROFILES_STORAGE_KEY));
  } catch {
    return emptyStore();
  }
}

/** Identity for a new profile. Never reuses a name, which the user may repeat. */
export function newAgentId(): string {
  const uuid = globalThis.crypto?.randomUUID?.();
  return uuid ?? `agent-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function agentForSession(store: AgentProfileStore,
    sessionKey: string): AgentProfile | undefined {
  const id = store.assignments[sessionKey];
  return id === undefined ? undefined : store.agents.find(agent => agent.id === id);
}

/** Prefer an unused character, including after deletions or manual edits. */
export function nextAgentColor(store: AgentProfileStore): AgentColor {
  const counts = new Map<AgentColor, number>(AGENT_COLORS.map(color => [color, 0]));
  for (const agent of store.agents) counts.set(agent.color, (counts.get(agent.color) ?? 0) + 1);
  return AGENT_COLORS.reduce((least, color) => counts.get(color)! < counts.get(least)! ? color : least);
}

/**
 * Why `engine` cannot be a teammate's provider, or null when it can. ACP
 * engines (Grok, Qwen, custom ACP profiles) cannot enforce Interface's approval
 * gate, so Rust refuses their threads and routines (docs/areas/agents.md); the
 * New agent form shows them disabled with this reason instead of accepting an
 * agent that can never start.
 */
export function agentEngineRefusal(engine: string): string | null {
  return engine.startsWith("acp:")
    ? "Can't be a teammate yet: Interface can't enforce its approval gate."
    : null;
}

/** The provider a new agent starts on: the last one used, unless it cannot be a teammate. */
export function newAgentEngine(lastEngine: string | null, fallback: string): string {
  return lastEngine !== null && agentEngineRefusal(lastEngine) === null ? lastEngine : fallback;
}

/**
 * The teammate's own gated start for a one-off task (protocol 38): Rust adds
 * the agent's job, home, folders and permission mode, so only the owner's
 * words, the folder and their launch choices travel. A model or effort picked
 * for another provider than the agent's is not sent.
 */
export function agentTaskCommand(agent: Pick<AgentProfile, "id" | "engine">,
    launch: { readonly engine: string; readonly cwd: string; readonly model?: string;
      readonly effort?: string },
    title: string, prompt: string, requestId: string,
): Extract<AgentCommand, { kind: "startAgentTask" }> {
  const sameProvider = launch.engine === agent.engine;
  const model = sameProvider ? launch.model?.trim() ?? "" : "";
  const effort = sameProvider ? launch.effort?.trim() ?? "" : "";
  const named = title.trim().split("\n")[0]?.trim() ?? "";
  return {
    kind: "startAgentTask", agentId: agent.id, text: prompt, cwd: launch.cwd, requestId,
    ...(named === "" ? {} : { title: named }),
    ...(model === "" || model === "default" ? {} : { model }),
    ...(effort === "" || effort === "default" ? {} : { effort }),
  };
}
