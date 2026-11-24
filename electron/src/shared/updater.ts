/** Serializable updater phases shared by the main and renderer processes. */
export const UPDATE_PHASES = [
  "disabled",
  "idle",
  "checking",
  "available",
  "downloading",
  "ready",
  "installing",
  "up-to-date",
  "error",
] as const;

export type UpdatePhase = typeof UPDATE_PHASES[number];

/** The feed class, never the feed URL (which may contain private routing data). */
export const UPDATE_SOURCES = ["local", "remote", "none"] as const;
export type UpdateSource = typeof UPDATE_SOURCES[number];

export const MAX_UPDATE_VERSION_LENGTH = 128;
export const MAX_UPDATE_ERROR_LENGTH = 512;

/**
 * The complete updater snapshot sent over IPC. It intentionally contains only
 * bounded JSON primitives so an updater or server cannot hand the renderer an
 * Error object, URL, byte counter, or other unbounded/native value.
 */
export interface UpdateState {
  readonly phase: UpdatePhase;
  readonly source: UpdateSource;
  readonly currentVersion: string;
  readonly availableVersion: string | null;
  /** Download completion in the inclusive range 0..100. */
  readonly progress: number | null;
  readonly lastCheckedAt: string | null;
  readonly error: string | null;
  readonly canCheck: boolean;
  /** True only after electron-updater reports the update as downloaded. */
  readonly canRestart: boolean;
}

function oneOf<const T extends readonly string[]>(values: T, value: unknown): value is T[number] {
  return typeof value === "string" && (values as readonly string[]).includes(value);
}

function boundedText(value: unknown, maxLength: number, allowEmpty = false): value is string {
  return typeof value === "string"
    && value.length <= maxLength
    && (allowEmpty || value.length > 0);
}

function nullableBoundedText(value: unknown, maxLength: number): value is string | null {
  return value === null || boundedText(value, maxLength);
}

function checkedAt(value: unknown): value is string | null {
  return value === null || (boundedText(value, 64) && Number.isFinite(Date.parse(value)));
}

/** Runtime guard for snapshots arriving across an IPC boundary. */
export function isUpdateState(value: unknown): value is UpdateState {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const state = value as Record<string, unknown>;
  const validProgress = state.progress === null
    || (typeof state.progress === "number" && Number.isFinite(state.progress)
      && state.progress >= 0 && state.progress <= 100);
  return oneOf(UPDATE_PHASES, state.phase)
    && oneOf(UPDATE_SOURCES, state.source)
    && boundedText(state.currentVersion, MAX_UPDATE_VERSION_LENGTH)
    && nullableBoundedText(state.availableVersion, MAX_UPDATE_VERSION_LENGTH)
    && validProgress
    && checkedAt(state.lastCheckedAt)
    && nullableBoundedText(state.error, MAX_UPDATE_ERROR_LENGTH)
    && typeof state.canCheck === "boolean"
    && typeof state.canRestart === "boolean"
    && state.canRestart === (state.phase === "ready");
}

/** Initial state for a process that has not yet received a usable feed. */
export function initialUpdateState(currentVersion: string): UpdateState {
  const version = boundedText(currentVersion, MAX_UPDATE_VERSION_LENGTH)
    ? currentVersion
    : "unknown";
  return {
    phase: "disabled",
    source: "none",
    currentVersion: version,
    availableVersion: null,
    progress: null,
    lastCheckedAt: null,
    error: null,
    canCheck: false,
    canRestart: false,
  };
}

/**
 * Fail closed when updater IPC is unavailable or returns a malformed snapshot.
 * The caller supplies product copy, never a raw native/updater error string.
 */
export function failedUpdateState(
  previous: UpdateState | null,
  message: string,
): UpdateState {
  const base = previous ?? initialUpdateState("unknown");
  const trimmed = message.trim();
  const error = (trimmed.length > 0 ? trimmed : "Update status is unavailable.")
    .slice(0, MAX_UPDATE_ERROR_LENGTH);
  return {
    ...base,
    phase: "error",
    availableVersion: null,
    progress: null,
    error,
    canCheck: false,
    canRestart: false,
  };
}

/**
 * The toolbar update pill's text, or null while there is nothing to update to.
 * It exists only for the three phases where an update is on its way.
 */
export function updateButtonLabel(state: UpdateState | null): string | null {
  if (state === null) return null;
  switch (state.phase) {
    case "available": return "Update available";
    case "downloading":
      return state.progress === null ? "Downloading update" : `Downloading ${Math.round(state.progress)}%`;
    case "ready": return "Restart to update";
    default: return null;
  }
}
