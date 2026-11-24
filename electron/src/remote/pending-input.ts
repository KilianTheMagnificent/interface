// Opaque command IDs only. Native views persist through a narrowly scoped reply bridge.
declare global {
  interface Window {
    __interfaceNativeInput?: { version: 1; pending: boolean };
    __interfaceConnectionVersion?: 5;
    webkit?: { messageHandlers?: {
      interfaceInput?: { postMessage(value: { operation: 'reserve' | 'clear'; key: string; command?: string }): Promise<unknown> };
      interfaceRequest?: { postMessage(value: import('./native-transport').NativeRequest): Promise<import('./native-transport').NativeReply> };
    } };
  }
}
export function hasPendingInput(key: string): boolean {
  if (typeof window !== 'undefined' && window.__interfaceConnectionVersion === 5) return window.__interfaceNativeInput?.pending !== false;
  try { return (typeof window !== 'undefined' && !!window.__interfaceNativeInput?.pending) || localStorage.getItem(key) !== null || sessionStorage.getItem(key) !== null; }
  catch { return true; }
}
async function nativeMarker(operation: 'reserve' | 'clear', key: string, command?: string): Promise<void> {
  if (typeof window === 'undefined' || !window.__interfaceNativeInput) return;
  const bridge = window.webkit?.messageHandlers?.interfaceInput;
  if (!bridge) throw new Error('Native input storage is unavailable.');
  await bridge.postMessage(command ? { operation, key, command } : { operation, key });
  window.__interfaceNativeInput.pending = operation === 'reserve';
}
export async function recordPendingInput(key: string, command: string): Promise<void> {
  if (window.__interfaceConnectionVersion !== 5) localStorage.setItem(key, command);
  await nativeMarker('reserve', key, command);
}
export async function clearPendingInput(key: string): Promise<void> {
  await nativeMarker('clear', key);
  if (window.__interfaceConnectionVersion !== 5) { localStorage.removeItem(key); sessionStorage.removeItem(key); }
}
