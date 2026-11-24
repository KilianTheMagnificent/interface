import { afterEach, describe, expect, it, vi } from 'vitest';
import fixture from '../fixtures/remote-access-v1.json';
import {
  NATIVE_OUTPUT_IDLE_AFTER, NATIVE_OUTPUT_IDLE_MAX_MS, NATIVE_OUTPUT_POLL_MS, NativeOutputSocket,
  nativeOutputPollDelay, nativeRequest,
} from './native-transport';
import { request } from './protocol';
import { hasPendingInput, recordPendingInput, clearPendingInput } from './pending-input';

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
function bridge(reply: ReturnType<typeof vi.fn>) {
  const native = { version: 1 as const, pending: false };
  const marker = vi.fn(async () => true);
  vi.stubGlobal('window', { __interfaceConnectionVersion: 5, __interfaceNativeInput: native,
    webkit: { messageHandlers: { interfaceRequest: { postMessage: reply }, interfaceInput: { postMessage: marker } } } });
  return { native, marker };
}
it('sends only bounded session arguments to native and never browser cookies or fetch', async () => {
  const native = vi.fn(async () => ({ version: 5, status: 202, body: '', contentType: 'application/octet-stream' }));
  bridge(native);
  const fetch = vi.fn(); vi.stubGlobal('fetch', fetch);
  const response = await request('/api/v1/terminals/fixture/1/input', { method: 'POST', body: new TextEncoder().encode('reply\r'), headers: { 'X-Interface-Control': 'lease', 'X-Interface-Command': 'command' } });
  expect(response.status).toBe(202); expect(fetch).not.toHaveBeenCalled();
  expect(native).toHaveBeenCalledExactlyOnceWith({ path: '/api/v1/terminals/fixture/1/input', method: 'POST', body: btoa('reply\r'), control: 'lease', command: 'command' });
});
it('keeps unknown input blocked without browser storage or any automatic retry', async () => {
  const native = vi.fn(async () => { throw new Error('Connection interrupted'); });
  const { marker } = bridge(native);
  vi.stubGlobal('localStorage', { setItem: () => { throw new Error('Opaque origin'); } });
  expect(hasPendingInput('fixture')).toBe(false);
  await recordPendingInput('fixture', 'command');
  await expect(request('/api/v1/terminals/fixture/1/input', { method: 'POST', body: 'reply' })).rejects.toThrow('Connection interrupted');
  expect(native).toHaveBeenCalledTimes(1); expect(hasPendingInput('fixture')).toBe(true);
  expect(marker).toHaveBeenCalledWith({ operation: 'reserve', key: 'fixture', command: 'command' });
  await clearPendingInput('fixture'); expect(hasPendingInput('fixture')).toBe(false);
});
it('waits for the renderer to consume output before reading the next cursor and stops on close', async () => {
  vi.useFakeTimers();
  const bytes = Uint8Array.from(fixture.output.hex.match(/../g)!, byte => parseInt(byte, 16));
  const encoded = btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join(''));
  const native = vi.fn().mockResolvedValueOnce({ version: 5, status: 200, body: encoded, contentType: 'application/octet-stream' })
    .mockResolvedValue({ version: 5, status: 204, body: '', contentType: 'application/octet-stream' });
  bridge(native);
  let consumed!: () => void;
  const socket = new NativeOutputSocket('/api/v1/terminals/fixture/1/stream?cursor=5');
  socket.onmessage = () => new Promise<void>(resolve => { consumed = resolve; });
  await vi.advanceTimersByTimeAsync(1000);
  expect(native).toHaveBeenCalledTimes(1);
  consumed(); await vi.advanceTimersByTimeAsync(200);
  expect(native.mock.calls[1][0].path).toBe('/api/v5/terminals/fixture/1/output?cursor=8');
  socket.close(); await vi.advanceTimersByTimeAsync(1000);
  expect(native).toHaveBeenCalledTimes(2);
});
it('decodes the passed-through base64 body byte for byte', async () => {
  const bytes = Uint8Array.from({ length: 256 }, (_, index) => index);
  const encoded = btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join(''));
  bridge(vi.fn(async () => ({ version: 5, status: 200, body: encoded, contentType: 'application/octet-stream' })));
  const response = await nativeRequest('/api/v2/workspaces');
  expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes);
});
it('refuses malformed native responses and cancelled requests before sending', async () => {
  const native = vi.fn(async () => ({ version: 6, status: 200, body: '', contentType: 'application/json' }));
  bridge(native);
  await expect(nativeRequest('/api/v2/workspaces')).rejects.toThrow('Invalid Mac response');
  native.mockResolvedValueOnce({ version: 5, status: 200, body: 'not base64!', contentType: 'application/json' });
  await expect(nativeRequest('/api/v2/workspaces')).rejects.toThrow('Invalid Mac response');
  const abort = new AbortController(); abort.abort();
  await expect(nativeRequest('/api/v2/workspaces', { signal: abort.signal })).rejects.toThrow();
  expect(native).toHaveBeenCalledTimes(2);
});

describe('idle native output backoff', () => {
  const empty = { version: 5, status: 204, body: '', contentType: 'application/octet-stream' };
  const page = () => {
    const bytes = Uint8Array.from(fixture.output.hex.match(/../g)!, byte => parseInt(byte, 16));
    return { version: 5, status: 200, body: btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join('')),
      contentType: 'application/octet-stream' };
  };
  // Read times, relative to the socket's first request.
  const timed = (native: ReturnType<typeof vi.fn>, start: number) =>
    native.mock.invocationCallOrder.map((_, index) => times[index] - start);
  let times: number[] = [];
  const recording = (replies: () => unknown) => vi.fn(async () => { times.push(Date.now()); return replies(); });

  it('backs off after repeated empty replies, doubling to a cap', async () => {
    vi.useFakeTimers(); times = [];
    const native = recording(() => empty);
    bridge(native);
    const start = Date.now();
    const socket = new NativeOutputSocket('/api/v1/terminals/fixture/1/stream?cursor=5');
    await vi.advanceTimersByTimeAsync(12_000);
    const gaps = timed(native, start).slice(1).map((at, index, all) => at - (index === 0 ? 0 : all[index - 1]));
    expect(gaps.slice(0, NATIVE_OUTPUT_IDLE_AFTER - 1)).toEqual(Array(NATIVE_OUTPUT_IDLE_AFTER - 1).fill(NATIVE_OUTPUT_POLL_MS));
    expect(gaps.slice(NATIVE_OUTPUT_IDLE_AFTER - 1, NATIVE_OUTPUT_IDLE_AFTER + 3)).toEqual([400, 800, 1600, 2000]);
    expect(Math.max(...gaps)).toBe(NATIVE_OUTPUT_IDLE_MAX_MS);
    expect(nativeOutputPollDelay(100)).toBe(NATIVE_OUTPUT_IDLE_MAX_MS);
    socket.close();
  });

  it('returns to the fast rate on any page, and on a wake without a second read', async () => {
    vi.useFakeTimers(); times = [];
    let reply: () => unknown = () => empty;
    const native = recording(() => reply());
    bridge(native);
    const socket = new NativeOutputSocket('/api/v1/terminals/fixture/1/stream?cursor=5');
    socket.onmessage = () => undefined;
    await vi.advanceTimersByTimeAsync(10_000);
    const backedOff = native.mock.calls.length;
    // Mid-backoff (2 s delay pending): input went to the Mac.
    socket.wake(); socket.wake();
    await vi.advanceTimersByTimeAsync(0);
    expect(native.mock.calls.length).toBe(backedOff + 1);
    await vi.advanceTimersByTimeAsync(NATIVE_OUTPUT_POLL_MS);
    expect(native.mock.calls.length).toBe(backedOff + 2);
    // Back off again, then a page (a gap page included) resets the delay.
    await vi.advanceTimersByTimeAsync(10_000);
    reply = page;
    await vi.advanceTimersByTimeAsync(NATIVE_OUTPUT_IDLE_MAX_MS);
    const afterPage = native.mock.calls.length;
    reply = () => empty;
    await vi.advanceTimersByTimeAsync(NATIVE_OUTPUT_POLL_MS);
    expect(native.mock.calls.length).toBe(afterPage + 1);
    socket.close();
  });

  it('stops polling when closed during a backoff', async () => {
    vi.useFakeTimers(); times = [];
    const native = recording(() => empty);
    bridge(native);
    const socket = new NativeOutputSocket('/api/v1/terminals/fixture/1/stream?cursor=5');
    await vi.advanceTimersByTimeAsync(10_000);
    const count = native.mock.calls.length;
    socket.close(); socket.wake();
    await vi.advanceTimersByTimeAsync(20_000);
    expect(native.mock.calls.length).toBe(count);
  });
});

describe('settled native output', () => {
  const empty = { version: 5, status: 204, body: '', contentType: 'application/octet-stream' };
  const page = () => {
    const bytes = Uint8Array.from(fixture.output.hex.match(/../g)!, byte => parseInt(byte, 16));
    return { version: 5, status: 200, body: btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join('')),
      contentType: 'application/octet-stream' };
  };

  it('reports settling once per burst of output, and for a first reply that is already empty', async () => {
    vi.useFakeTimers();
    const replies = ['empty', 'empty', 'page', 'page', 'empty', 'empty', 'empty', 'page', 'empty'];
    let next = 0;
    bridge(vi.fn(async () => replies[Math.min(next++, replies.length - 1)] === 'page' ? page() : empty));
    const socket = new NativeOutputSocket('/api/v1/terminals/fixture/1/stream?cursor=5');
    const events: string[] = [];
    socket.onmessage = () => { events.push('page'); };
    socket.onsettled = () => { events.push('settled'); };
    await vi.advanceTimersByTimeAsync(10_000);
    expect(events).toEqual(['settled', 'page', 'page', 'settled', 'page', 'settled']);
    socket.close();
  });

  it('does not report settling after close', async () => {
    vi.useFakeTimers();
    let answer!: (reply: unknown) => void;
    bridge(vi.fn(() => new Promise(resolve => { answer = resolve; })));
    const socket = new NativeOutputSocket('/api/v1/terminals/fixture/1/stream?cursor=5');
    const settled = vi.fn();
    socket.onsettled = settled;
    await vi.advanceTimersByTimeAsync(0);
    socket.close();
    answer(empty);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(settled).not.toHaveBeenCalled();
  });
});
