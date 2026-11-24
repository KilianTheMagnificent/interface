/**
 * The one plain-object narrowing every main-process boundary uses before it
 * reads a field off untrusted JSON: host frames, updater configuration, native
 * payloads, and the terminal bridge handshake.
 *
 * Arrays are refused rather than accepted as index-keyed records, so a payload
 * that arrives as `[]` cannot satisfy a check written for a named object.
 * `undefined` — not null — is the one "not a named object" answer, so callers
 * can use optional chaining without a second falsy case to remember.
 *
 * The sandboxed preload keeps its own copy: it cannot import a relative module
 * at runtime.
 */
export function plainObject(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}
