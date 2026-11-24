import { TERMINAL_LAUNCH_PROFILE_IDS, type TerminalLaunchProfileId } from "../shared/ipc";

export const TERMINAL_PROFILE_MEMORY_KEY = "interface.integrated-terminals.profile-memory.v1";

const PROFILE_MEMORY_VERSION = 1;
const MAX_PROFILE_MEMORY_ENTRIES = 256;
const MAX_PROFILE_MEMORY_BYTES = 64 * 1024;
const MAX_TERMINAL_ID_LENGTH = 128;
const PROFILE_IDS: ReadonlySet<string> = new Set(TERMINAL_LAUNCH_PROFILE_IDS);

export interface TerminalProfileMemoryStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function terminalId(value: unknown): value is string {
  return typeof value === "string" && value.length > 0
    && value.length <= MAX_TERMINAL_ID_LENGTH
    && !/[\u0000-\u001f\u007f]/u.test(value);
}

function profileId(value: unknown): value is TerminalLaunchProfileId {
  return typeof value === "string" && PROFILE_IDS.has(value);
}

/** Tolerant, bounded decoder for presentation-only terminal provider marks. */
export function parseTerminalProfileMemory(raw: string | null): ReadonlyMap<
  string, TerminalLaunchProfileId
> {
  if (raw === null || raw.length > MAX_PROFILE_MEMORY_BYTES) return new Map();
  try {
    const decoded: unknown = JSON.parse(raw);
    if (decoded === null || typeof decoded !== "object" || Array.isArray(decoded)) {
      return new Map();
    }
    const record = decoded as { version?: unknown; entries?: unknown };
    if (record.version !== PROFILE_MEMORY_VERSION || !Array.isArray(record.entries)
        || record.entries.length > MAX_PROFILE_MEMORY_ENTRIES) {
      return new Map();
    }
    const result = new Map<string, TerminalLaunchProfileId>();
    for (const entry of record.entries) {
      if (!Array.isArray(entry) || entry.length !== 2
          || !terminalId(entry[0]) || !profileId(entry[1]) || result.has(entry[0])) {
        return new Map();
      }
      result.set(entry[0], entry[1]);
    }
    return result;
  } catch {
    return new Map();
  }
}

export function readTerminalProfileMemory(
    storage: TerminalProfileMemoryStorage): ReadonlyMap<string, TerminalLaunchProfileId> {
  try {
    return parseTerminalProfileMemory(storage.getItem(TERMINAL_PROFILE_MEMORY_KEY));
  } catch {
    return new Map();
  }
}

/** Keep only identities still claimed by the live registry or a saved workspace. */
export function pruneTerminalProfileMemory(
    profiles: ReadonlyMap<string, TerminalLaunchProfileId>,
    retainedTerminalIds: ReadonlySet<string>): ReadonlyMap<string, TerminalLaunchProfileId> {
  if ([...profiles.keys()].every(id => retainedTerminalIds.has(id))) return profiles;
  return new Map([...profiles].filter(([id]) => retainedTerminalIds.has(id)));
}

/** A blocked or full storage area must never block terminal creation. */
export function writeTerminalProfileMemory(storage: TerminalProfileMemoryStorage,
    profiles: ReadonlyMap<string, TerminalLaunchProfileId>): void {
  try {
    if (profiles.size === 0) {
      storage.removeItem(TERMINAL_PROFILE_MEMORY_KEY);
      return;
    }
    const entries = [...profiles].filter(([id, profile]) => terminalId(id) && profileId(profile))
      .slice(-MAX_PROFILE_MEMORY_ENTRIES);
    storage.setItem(TERMINAL_PROFILE_MEMORY_KEY, JSON.stringify({
      version: PROFILE_MEMORY_VERSION,
      entries,
    }));
  } catch {
    // Provider marks are presentation only. Keep the live in-memory value when
    // privacy or quota settings make localStorage unavailable.
  }
}
