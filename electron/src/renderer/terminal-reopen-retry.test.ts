import { describe, expect, it, vi } from 'vitest';
import type { CreateTerminalRequest, TerminalSummary } from '../shared/ipc';
import { encodeIpcErrorMessage } from '../shared/ipc-error';
import { reopenWithRetry, REOPEN_RETRY_BACKOFF_MS } from './terminal-reopen-retry';

/** A host refusal as `window.iface.createTerminal` rejects with it: the words
 * and the v1 error shape the bridge appends (shared/ipc-error.ts). */
const refusal = (message: string, code: string, retryable: boolean, replaySafe: boolean): Error =>
  new Error(encodeIpcErrorMessage(message, { code, retryable, replaySafe }));

const chat: CreateTerminalRequest = { id: 'pane', cwd: '/repo', title: 'Codex',
  launch: { version: 1, profileId: 'codex' },
  launchSession: { version: 1, id: 'sess-1', resume: true } };

const summary = (id: string): TerminalSummary => ({ id, generation: 1, title: 'Codex',
  root: '/repo', cwd: '/repo', sessionKey: 'codex#sess-1', state: 'running',
  size: { columns: 80, rows: 24 }, createdAt: '', updatedAt: '',
  launch: { version: 1, profileId: 'codex' } });

const noShell = () => [];
const instantSleep = () => Promise.resolve();

describe('reopenWithRetry', () => {
  it('returns the terminal on the first attempt without sleeping', async () => {
    const sleep = vi.fn(instantSleep);
    const result = await reopenWithRetry(chat, {
      createTerminal: async () => summary('pane'), rebuild: async () => undefined,
      shellFallbacks: noShell, sleep, cancelled: () => false });
    expect(result).toEqual({ ok: true, terminal: summary('pane'), launchSession: chat.launchSession });
    expect(sleep).not.toHaveBeenCalled();
  });

  it('retries transient host-restart refusals and succeeds, rebuilding from the settled roster', async () => {
    let calls = 0;
    const rebuilt: CreateTerminalRequest = { ...chat, cwd: '/repo' };
    const rebuild = vi.fn(async () => rebuilt);
    const result = await reopenWithRetry(chat, {
      createTerminal: async req => {
        calls += 1;
        if (calls <= 2) throw refusal('Try again.', 'terminal-authority-unavailable', true, true);
        expect(req).toBe(rebuilt); // the retry used the rebuilt request, not the stale one
        return summary('pane');
      },
      rebuild, shellFallbacks: noShell, sleep: instantSleep, cancelled: () => false });
    expect(result.ok).toBe(true);
    expect(calls).toBe(3);
    expect(rebuild).toHaveBeenCalledTimes(2);
  });

  it('gives up after the bounded rounds and reports the last error for a chat pane', async () => {
    let calls = 0;
    const result = await reopenWithRetry(chat, {
      createTerminal: async () => { calls += 1; throw new Error('still refused'); },
      rebuild: async () => undefined, shellFallbacks: noShell, sleep: instantSleep, cancelled: () => false });
    expect(result).toEqual({ ok: false, cancelled: false, error: new Error('still refused') });
    // one initial attempt + one per backoff step
    expect(calls).toBe(REOPEN_RETRY_BACKOFF_MS.length + 1);
  });

  it('retries each transient authority refusal the host names, as the host raises it', async () => {
    // terminal.rs / native.rs raise these replay-safe and, all but the first,
    // not retryable: the transient code list is what lets them through.
    const refusals = [
      refusal('Interface could not read the current terminal workspaces.',
        'terminal-authority-unavailable', true, true),
      refusal('Interface could not read the current terminal workspaces.',
        'terminal-authority-unavailable', false, true),
      refusal('That saved terminal has no host restart proof.',
        'terminal-reopen-authority-unproven', false, true),
      refusal('That saved terminal no longer matches its host restart proof.',
        'terminal-reopen-authority-mismatch', false, true),
    ];
    // As each arrives after the preload, and still inside Electron's wrapper.
    for (const failure of refusals.flatMap(refused => [refused,
      new Error(`Error invoking remote method 'iface:createTerminal': Error: ${refused.message}`)])) {
      let calls = 0;
      const result = await reopenWithRetry(chat, {
        createTerminal: async () => {
          calls += 1;
          if (calls === 1) throw failure;
          return summary('pane');
        },
        rebuild: async () => undefined, shellFallbacks: noShell, sleep: instantSleep, cancelled: () => false });
      expect(result.ok).toBe(true);
      expect(calls).toBe(2);
    }
  });

  it('retries a refusal the host marks retryable, and never one it marks unsafe to replay', async () => {
    const attempts = async (failure: Error): Promise<number> => {
      let calls = 0;
      await reopenWithRetry(chat, {
        createTerminal: async () => { calls += 1; throw failure; },
        rebuild: async () => undefined, shellFallbacks: noShell, sleep: instantSleep, cancelled: () => false });
      return calls;
    };
    expect(await attempts(refusal('Try again.', 'terminal-authority-busy', true, true)))
      .toBe(REOPEN_RETRY_BACKOFF_MS.length + 1);
    // An ambiguous launch is never replayed, whatever its code says.
    expect(await attempts(refusal('The host exited.', 'terminal-authority-unavailable', true, false)))
      .toBe(1);
    expect(await attempts(refusal('The Rust host stopped unexpectedly (exit 1).', 'host-exited',
      true, false))).toBe(1);
    expect(await attempts(refusal('The Rust host was stopped by the application.', 'host-stopped',
      true, false))).toBe(1);
    // A marker this build cannot read might say anything: no replay either.
    expect(await attempts(new Error('Later. [iface-error/2 code=terminal-authority-unavailable]')))
      .toBe(1);
  });

  it('does not retry a refusal whose code is not transient, and goes straight to the shell fallback', async () => {
    const refused = refusal('That terminal id is already in use.', 'terminal-id-conflict', false, true);
    const sleep = vi.fn(instantSleep);
    const rebuild = vi.fn(async () => chat);
    const createTerminal = vi.fn(async (_request: CreateTerminalRequest): Promise<TerminalSummary> => {
      throw refused;
    });
    const chatResult = await reopenWithRetry(chat, {
      createTerminal, rebuild, shellFallbacks: noShell, sleep, cancelled: () => false });
    expect(chatResult).toEqual({ ok: false, cancelled: false, error: refused });
    expect(createTerminal).toHaveBeenCalledTimes(1);
    expect(sleep).not.toHaveBeenCalled();
    expect(rebuild).not.toHaveBeenCalled();

    const shell: CreateTerminalRequest = { id: 'pane', cwd: '/repo', title: 'Terminal' };
    const attempts: CreateTerminalRequest[] = [];
    const shellResult = await reopenWithRetry({ id: 'pane', cwd: '/repo', title: 'Codex' }, {
      createTerminal: async req => {
        attempts.push(req);
        if (req === shell) return summary('pane');
        throw refused;
      },
      rebuild, shellFallbacks: () => [shell], sleep, cancelled: () => false });
    expect(shellResult.ok).toBe(true);
    expect(attempts).toEqual([{ id: 'pane', cwd: '/repo', title: 'Codex' }, shell]);
    expect(sleep).not.toHaveBeenCalled();
  });

  it('keeps the bounded retry for a refusal without the host\'s facts', async () => {
    // Main's own refusal (the terminal data channel still connecting) and an
    // older build's host refusal arrive as words alone: still the transient
    // window eb55c32 retries through.
    for (const message of [
      'The integrated terminal data channel is still connecting. Interface did not start a terminal.',
      'Interface could not read the current terminal workspaces.',
    ]) {
      let calls = 0;
      const result = await reopenWithRetry(chat, {
        createTerminal: async () => {
          calls += 1;
          if (calls <= 2) throw new Error(message);
          return summary('pane');
        },
        rebuild: async () => undefined, shellFallbacks: noShell, sleep: instantSleep, cancelled: () => false });
      expect(result.ok).toBe(true);
      expect(calls).toBe(3);
    }
  });

  it('falls back to a plain shell for a non-chat pane after the retries fail', async () => {
    const shell: CreateTerminalRequest = { id: 'pane', cwd: '/repo', title: 'Terminal' };
    let chatAttempts = 0;
    const result = await reopenWithRetry({ id: 'pane', cwd: '/repo', title: 'Terminal' }, {
      createTerminal: async req => {
        if (req.title === 'Terminal' && req.launchSession === undefined && chatAttempts >= 1 + REOPEN_RETRY_BACKOFF_MS.length) {
          return summary('pane');
        }
        chatAttempts += 1;
        throw new Error('no');
      },
      rebuild: async () => undefined, shellFallbacks: () => [shell], sleep: instantSleep, cancelled: () => false });
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.launchSession).toBeUndefined();
  });

  it('stops retrying when cancelled during backoff', async () => {
    let calls = 0;
    let cancel = false;
    const result = await reopenWithRetry(chat, {
      createTerminal: async () => { calls += 1; throw new Error('refused'); },
      rebuild: async () => chat, shellFallbacks: noShell,
      sleep: async () => { cancel = true; }, cancelled: () => cancel });
    expect(result).toEqual({ ok: false, cancelled: true, error: new Error('refused') });
    expect(calls).toBe(1);
  });

  it('reports a failed roster read without replaying the stale launch', async () => {
    const failure = new Error('Host roster unavailable');
    const createTerminal = vi.fn(async () => { throw new Error('refused'); });
    const result = await reopenWithRetry(chat, {
      createTerminal, rebuild: async () => { throw failure; }, shellFallbacks: noShell,
      sleep: instantSleep, cancelled: () => false });
    expect(result).toEqual({ ok: false, cancelled: false, error: failure });
    expect(createTerminal).toHaveBeenCalledTimes(1);
  });

  it('does not retry a pane removed while the roster read was in flight', async () => {
    let cancel = false;
    const createTerminal = vi.fn(async () => { throw new Error('refused'); });
    const result = await reopenWithRetry(chat, {
      createTerminal, rebuild: async () => { cancel = true; return chat; }, shellFallbacks: noShell,
      sleep: instantSleep, cancelled: () => cancel });
    expect(result).toEqual({ ok: false, cancelled: true, error: new Error('refused') });
    expect(createTerminal).toHaveBeenCalledTimes(1);
  });
});
