import { splitIpcErrorMessage, type IpcErrorShape } from "../shared/ipc-error";

function rawText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

// Electron reports a main-process rejection as "Error invoking remote method
// 'iface:setModel': Error: <the real message>". Only the tail is worth putting
// in front of a user, and never the error-shape marker a host error carries.
export function ipcMessage(err: unknown): string {
  const raw = splitIpcErrorMessage(rawText(err)).message;
  const at = raw.lastIndexOf("Error: ");
  return at < 0 ? raw : raw.slice(at + "Error: ".length);
}

/** Text shown as it came, minus the error-shape marker a host error carries. */
export function withoutIpcErrorShape(text: string): string {
  return splitIpcErrorMessage(text).message;
}

/**
 * The host's typed facts for a rejected request (shared/ipc-error.ts):
 * `typed` for a Rust host error, `unrecognized` for another version's marker,
 * `untyped` for everything else — main's own refusals, renderer errors, and a
 * rejection from a build that predates the shape.
 */
export function ipcErrorShape(err: unknown): IpcErrorShape {
  return splitIpcErrorMessage(rawText(err)).shape;
}
