export interface OutputFrame { start: number; end: number; gap: boolean; bytes: Uint8Array }

export function decodeOutput(buffer: ArrayBuffer): OutputFrame {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < 21 || String.fromCharCode(...bytes.subarray(0, 4)) !== "IRP1") {
    throw new Error("Unsupported terminal stream. Reopen Interface on your Mac.");
  }
  const view = new DataView(buffer);
  const start = Number(view.getBigUint64(4, true));
  const end = Number(view.getBigUint64(12, true));
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || end < start
    || end - start !== bytes.length - 21 || bytes[20] > 1) throw new Error("Invalid terminal stream.");
  return { start, end, gap: bytes[20] === 1, bytes: bytes.subarray(21) };
}

export class RemoteFailure extends Error {
  constructor(message: string, readonly replaySafe: boolean, readonly status: number) { super(message); }
}

export async function request(path: string, init?: RequestInit): Promise<Response> {
  const response = usesNativeTransport() ? await nativeRequest(path, init)
    : await fetch(path, { ...init, cache: "no-store", credentials: "same-origin", signal: init?.signal ?? AbortSignal.timeout(10_000) });
  if (!response.ok) {
    let error: { message?: string; replaySafe?: boolean } = {};
    try { error = await response.json(); } catch { /* A closed gateway need not send JSON. */ }
    throw new RemoteFailure(error.message ?? (response.status === 401
      ? "Pair this phone from Interface on your Mac." : "The Mac could not accept this request."), error.replaySafe === true, response.status);
  }
  return response;
}
import { nativeRequest, usesNativeTransport } from './native-transport';
