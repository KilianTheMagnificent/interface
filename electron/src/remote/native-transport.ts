import { decodeOutput } from './protocol';

export interface NativeRequest {
  path: string;
  method: string;
  body: string;
  control?: string;
  command?: string;
}
export interface NativeReply { version: 5; status: number; body: string; contentType: string }
export function usesNativeTransport(): boolean { return typeof window !== 'undefined' && window.__interfaceConnectionVersion === 5; }

export async function nativeRequest(path: string, init?: RequestInit): Promise<Response> {
  const bridge = window.webkit?.messageHandlers?.interfaceRequest;
  if (!bridge) throw new Error('The secure connection is unavailable. Reopen Interface.');
  const signal = init?.signal;
  signal?.throwIfAborted();
  const headers = new Headers(init?.headers);
  const bytes = init?.body ? new Uint8Array(await new Response(init.body).arrayBuffer()) : new Uint8Array();
  if (bytes.length > 16_384) throw new Error('This message is too large.');
  const encoded = btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join(''));
  const request: NativeRequest = { path, method: init?.method ?? 'GET', body: encoded };
  const control = headers.get('X-Interface-Control'); const command = headers.get('X-Interface-Command');
  if (control) request.control = control;
  if (command) request.command = command;
  const reply = await bridge.postMessage(request);
  signal?.throwIfAborted();
  if (reply.version !== 5 || !Number.isInteger(reply.status) || reply.status < 100 || reply.status > 599
    || typeof reply.body !== 'string' || reply.body.length > 3 * 1024 * 1024) throw new Error('Invalid Mac response.');
  const decoded = decodeBase64(reply.body);
  return new Response(reply.status === 204 ? null : decoded, { status: reply.status, headers: { 'Content-Type': reply.contentType } });
}

/** Native passes the tunnel's base64 body through unchanged. A plain indexed
 * loop avoids a callback per character on multi-megabyte history replies
 * (Uint8Array.fromBase64 needs iOS 18.2; the app supports iOS 17). */
function decodeBase64(body: string): Uint8Array<ArrayBuffer> {
  let binary: string;
  try { binary = atob(body); } catch { throw new Error('Invalid Mac response.'); }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

/** Output is read every 200 ms while it flows. After this many empty (204)
 * replies in a row the delay doubles up to NATIVE_OUTPUT_IDLE_MAX_MS, so an
 * idle view stops making five encrypted round trips a second. */
export const NATIVE_OUTPUT_POLL_MS = 200;
export const NATIVE_OUTPUT_IDLE_AFTER = 5;
export const NATIVE_OUTPUT_IDLE_MAX_MS = 2_000;
export function nativeOutputPollDelay(emptyReplies: number): number {
  if (emptyReplies < NATIVE_OUTPUT_IDLE_AFTER) return NATIVE_OUTPUT_POLL_MS;
  return Math.min(NATIVE_OUTPUT_IDLE_MAX_MS,
    NATIVE_OUTPUT_POLL_MS * 2 ** (emptyReplies - NATIVE_OUTPUT_IDLE_AFTER + 1));
}

/** Native output uses serial encrypted reads. The rendered cursor is committed
 * only after the same xterm write callback used by the legacy WebSocket path. */
export class NativeOutputSocket {
  readyState = 0;
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: ArrayBuffer }) => unknown) | null = null;
  onclose: (() => void) | null = null;
  /** The first empty reply after output, or the first reply of all when it
   * is empty: everything the Mac had written is rendered, so the screen now
   * is the Mac's current one. Repeated empty replies do not call it again. */
  onsettled: (() => void) | null = null;
  private timer?: ReturnType<typeof setTimeout>;
  private finishDelay?: () => void;
  private emptyReplies = 0;
  private settled = false;
  private readonly abort = new AbortController();
  constructor(private readonly path: string) { this.timer = setTimeout(() => { void this.poll(); }, 0); }
  close(): void {
    if (this.readyState === 3) return;
    this.readyState = 3; this.abort.abort(); clearTimeout(this.timer); this.finishDelay?.(); this.finishDelay = undefined; this.onclose?.();
  }
  /** Input or an approval just reached the Mac, so its echo is due: poll at
   * the fast rate again. Ends a pending delay only; never starts a second read. */
  wake(): void {
    this.emptyReplies = 0;
    if (this.finishDelay === undefined) return;
    clearTimeout(this.timer);
    this.finishDelay();
  }
  private async poll(): Promise<void> {
    let path = this.path.replace('/api/v1/', '/api/v5/').replace('/stream?', '/output?');
    try {
      while (this.readyState !== 3) {
        const response = await nativeRequest(path, { signal: this.abort.signal });
        if (!response.ok) throw new Error('Session output is unavailable.');
        if (this.readyState === 0) { this.readyState = 1; this.onopen?.(); }
        if (response.status !== 204) {
          // Any page, a gap included, is output flowing again.
          this.emptyReplies = 0;
          const data = await response.arrayBuffer();
          await this.onmessage?.({ data });
          path = path.replace(/cursor=\d+$/, `cursor=${decodeOutput(data).end}`);
          this.settled = false;
        } else {
          this.emptyReplies++;
          if (!this.settled && this.readyState !== 3) { this.settled = true; this.onsettled?.(); }
        }
        if (this.readyState === 3) break;
        const delay = nativeOutputPollDelay(this.emptyReplies);
        await new Promise<void>(resolve => { this.finishDelay = resolve; this.timer = setTimeout(resolve, delay); });
        this.finishDelay = undefined;
      }
    } catch { this.close(); }
  }
}
export function outputSocket(path: string): WebSocket | NativeOutputSocket {
  if (usesNativeTransport()) return new NativeOutputSocket(path);
  const url = new URL(path, location.href);
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return new WebSocket(url);
}
