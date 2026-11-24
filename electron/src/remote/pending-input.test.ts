import { afterEach, expect, it, vi } from 'vitest';
import { hasPendingInput, recordPendingInput, clearPendingInput } from './pending-input';
afterEach(() => vi.unstubAllGlobals());
function storage() { const map = new Map<string,string>(); return { getItem: (k:string) => map.get(k) ?? null, setItem: (k:string,v:string) => { map.set(k,v); }, removeItem: (k:string) => { map.delete(k); } }; }
it('waits for native durable reservation and preserves uncertainty when native storage refuses', async () => {
  vi.stubGlobal('localStorage',storage()); vi.stubGlobal('sessionStorage',storage());
  let release: ((value: boolean) => void) | undefined;
  const postMessage = vi.fn(() => new Promise(resolve => { release = resolve; }));
  vi.stubGlobal('window',{ __interfaceNativeInput: {version:1,pending:false}, webkit: {messageHandlers:{interfaceInput:{postMessage}}} });
  let reserved = false;
  const write = recordPendingInput('key','opaque-id').then(() => { reserved = true; });
  expect(reserved).toBe(false); expect(hasPendingInput('key')).toBe(true);
  release!(true); await write; expect(reserved).toBe(true);
  postMessage.mockImplementationOnce(async () => { throw new Error('storage unavailable'); });
  await expect(clearPendingInput('key')).rejects.toThrow(); expect(hasPendingInput('key')).toBe(true);
});
it('restores a native marker after web process replacement and clears both stores only after acknowledgement', async () => {
  vi.stubGlobal('localStorage',storage()); vi.stubGlobal('sessionStorage',storage());
  vi.stubGlobal('window',{ __interfaceNativeInput: {version:1,pending:true}, webkit:{messageHandlers:{interfaceInput:{postMessage:async () => true}}} });
  expect(hasPendingInput('key')).toBe(true); await clearPendingInput('key'); expect(hasPendingInput('key')).toBe(false);
});
