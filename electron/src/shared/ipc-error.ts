/**
 * The main → renderer error shape, version 1.
 *
 * Electron gives a renderer only the text of a main-process rejection:
 * `ipcMain.handle` replies with `error.toString()`, and contextBridge copies an
 * Error's message and drops every other property. A Rust host error's code,
 * retryable and replaySafe therefore used to stop at the main process, and the
 * renderer had to guess an effect's outcome from its wording. The bridge
 * (`main/rust-ipc-bridge.ts`) now appends them to the message as one trailing
 * marker:
 *
 *     <message> [iface-error/1 code=<code> retryable=<bool> replay-safe=<bool>]
 *
 * The preload keeps the marker while it strips Electron's wrapper (it cannot
 * import this module, so it matches the same tail itself); the renderer reads
 * it (`ipc-message.ts`) and never shows it.
 *
 * Only a Rust host error carries it. A rejection without one — main's own
 * refusals, Electron's, or one from a build that predates this shape — is
 * `untyped`, and a marker of another version or a malformed one is
 * `unrecognized`. Neither proves anything about a provider effect.
 */

export const IPC_ERROR_SHAPE_VERSION = 1;

/** The Rust host's typed error facts (`RustHostError`, the wire's `error` object). */
export interface IpcErrorFacts {
  readonly code: string;
  readonly retryable: boolean;
  readonly replaySafe: boolean;
}

export type IpcErrorShape =
  | ({ readonly kind: "typed" } & IpcErrorFacts)
  | { readonly kind: "unrecognized" }
  | { readonly kind: "untyped" };

/**
 * The host's delivery-effect codes (`apps/interface-host/src/dispatcher.rs`).
 * Only `effect-definitely-unsent` is replay-safe; `effect-definitely-rejected`
 * is not replay-safe but still proves the provider never took the effect.
 */
export const EFFECT_DEFINITELY_UNSENT_CODE = "effect-definitely-unsent";
export const EFFECT_DEFINITELY_REJECTED_CODE = "effect-definitely-rejected";
export const EFFECT_OUTCOME_UNKNOWN_CODE = "effect-outcome-unknown";

const CODE = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$/u;
/** Any version's marker, so an unknown one is still kept out of sight. */
const MARKER = /\s*\[iface-error\/(\d{1,6})(?: ([^\]\n]*))?\]$/u;
const V1_FIELDS =
  /^code=([A-Za-z0-9][A-Za-z0-9_.-]{0,95}) retryable=(true|false) replay-safe=(true|false)$/u;
/** A code the marker cannot carry keeps its flags under this name. */
const UNREPRESENTABLE_CODE = "unrepresentable-code";

const UNTYPED: IpcErrorShape = { kind: "untyped" };
const UNRECOGNIZED: IpcErrorShape = { kind: "unrecognized" };

/** The message the bridge throws to the renderer for a Rust host error. */
export function encodeIpcErrorMessage(message: string, facts: IpcErrorFacts): string {
  const code = CODE.test(facts.code) ? facts.code : UNREPRESENTABLE_CODE;
  const marker = `[iface-error/${String(IPC_ERROR_SHAPE_VERSION)} code=${code} `
    + `retryable=${String(facts.retryable)} replay-safe=${String(facts.replaySafe)}]`;
  return message === "" ? marker : `${message} ${marker}`;
}

/** Separate a received message into the words for a person and the shape. */
export function splitIpcErrorMessage(text: string): { message: string; shape: IpcErrorShape } {
  const marker = MARKER.exec(text);
  if (marker === null) return { message: text, shape: UNTYPED };
  const message = text.slice(0, marker.index);
  if (marker[1] !== String(IPC_ERROR_SHAPE_VERSION)) return { message, shape: UNRECOGNIZED };
  const fields = V1_FIELDS.exec(marker[2] ?? "");
  if (fields === null) return { message, shape: UNRECOGNIZED };
  return {
    message,
    shape: {
      kind: "typed",
      code: fields[1],
      retryable: fields[2] === "true",
      replaySafe: fields[3] === "true",
    },
  };
}
