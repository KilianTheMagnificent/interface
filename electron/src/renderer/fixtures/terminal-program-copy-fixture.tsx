import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import { TerminalView, type TerminalViewHandle } from '../components/TerminalView';
import { TERMINAL_PROGRAM_COPY_KEY } from '../terminal-keys';
import { MAX_REPLAY_OUTPUT_BYTES, TerminalReplayBuffer } from '../terminal-replay-buffer';
import { assert, untilWithin } from './fixture-support';

const until = untilWithin({ pollMs: 10 });

/** Codex 0.157's startup: kitty flags on the main screen, then the alternate
 * screen with pointer capture and its own flags. */
const CODEX_STARTUP = '\x1b[?2004h\x1b[>7u\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?1003h\x1b[>7u';
const CODEX_EXIT = '\x1b[<u\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1049l\x1b[<u';

/** ⌘C in a pane whose program draws its own selection reaches that program as
 * Super+C, through the production xterm and its parser; xterm's own selection
 * and programs that never asked for kitty key reports keep ordinary copy. */
export async function terminalProgramCopyFixture(root: Root): Promise<Record<string, unknown>> {
  let sent = '';
  const mount = async (terminalId: string): Promise<TerminalViewHandle> => {
    let handle: TerminalViewHandle | null = null;
    flushSync(() => root.render(null));
    flushSync(() => root.render(<div style={{ width: 420, height: 240 }}>
      <TerminalView terminalId={terminalId} active onData={data => { sent += data; }} onReady={ready => {
        if (ready) { handle = ready; ready.completeSetup(); }
      }} />
    </div>));
    await until(() => handle !== null, `${terminalId} terminal did not mount`);
    return handle!;
  };
  const write = (view: TerminalViewHandle, data: string): Promise<void> =>
    new Promise(resolve => view.write(data, resolve));
  const commandC = (): string => {
    sent = '';
    const textarea = document.querySelector<HTMLTextAreaElement>('.xterm-helper-textarea')!;
    textarea.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'c', code: 'KeyC', keyCode: 67, metaKey: true, bubbles: true, cancelable: true,
    }));
    return sent;
  };
  const checks: string[] = [];
  const expectCopy = (expected: string, message: string): void => {
    const received = commandC();
    assert(received === expected, `${message}: sent ${JSON.stringify(received)}`);
    checks.push(message);
  };

  const view = await mount('program-copy');
  view.focus();
  await write(view, '$ codex\r\n');
  expectCopy('', 'A shell with nothing selected sends nothing');
  await write(view, CODEX_STARTUP);
  expectCopy(TERMINAL_PROGRAM_COPY_KEY, 'Codex receives Super+C for its own selection');
  await write(view, '\x1b[<u');
  expectCopy('', 'A pointer-capturing program without key reports keeps ordinary copy');
  await write(view, '\x1b[>1u');
  expectCopy(TERMINAL_PROGRAM_COPY_KEY, 'A re-pushed disambiguate flag forwards again');
  await write(view, CODEX_EXIT);
  expectCopy('', 'The shell after Codex exits sends nothing');

  // A remount after a long session gets only the tracked modes, not the startup bytes.
  const tail = new TerminalReplayBuffer();
  tail.push(new TextEncoder().encode(CODEX_STARTUP + 'x'.repeat(MAX_REPLAY_OUTPUT_BYTES)));
  assert(tail.truncated, 'Replay tail kept Codex startup bytes');
  const remounted = await mount('program-copy-remount');
  remounted.focus();
  await write(remounted, tail.modes.restoreSequence);
  expectCopy(TERMINAL_PROGRAM_COPY_KEY, 'A remounted Codex pane still forwards ⌘C');

  // A search match is xterm's own selection, which ⌘C copies itself.
  await write(remounted, 'find this text');
  assert(remounted.findNext('find this'), 'Search did not select the match');
  expectCopy('', 'xterm\'s own selection keeps ⌘C');

  flushSync(() => root.render(null));
  return { checks };
}
