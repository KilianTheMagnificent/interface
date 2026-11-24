import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import { TerminalView, type TerminalViewHandle } from '../components/TerminalView';
import { assert, untilWithin } from './fixture-support';

const until = untilWithin({ pollMs: 10 });

/** The default cursor and a program's bar are drawn as the reference blue caret,
 * inset within the row, while block cursors a program asks for, hidden cursors
 * and unfocused panes stay xterm's. */
export async function terminalCaretFixture(root: Root): Promise<Record<string, unknown>> {
  let handle: TerminalViewHandle | null = null;
  flushSync(() => root.render(null));
  flushSync(() => root.render(<div style={{ width: 420, height: 240 }}>
    <TerminalView terminalId="caret" active onData={() => {}} onReady={ready => {
      if (ready) { handle = ready; ready.completeSetup(); }
    }} />
  </div>));
  await until(() => handle !== null, 'Caret terminal did not mount');
  const view = handle!;
  const host = document.querySelector<HTMLElement>('[data-terminal-id="caret"]')!;
  const screen = host.querySelector<HTMLElement>('.xterm-screen')!;
  const caret = (): HTMLElement | null => host.querySelector<HTMLElement>('.terminal-caret');
  const write = (data: string): Promise<void> => new Promise(resolve => view.write(data, resolve));
  const frames = (): Promise<void> => new Promise(resolve =>
    requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  const shown = async (expected: boolean, message: string): Promise<void> => {
    await frames();
    await until(() => (caret()?.hidden === false) === expected, message);
  };
  const cell = (): { width: number; height: number; left: number; top: number } => {
    const bounds = screen.getBoundingClientRect();
    const size = view.size()!;
    return { width: bounds.width / size.columns, height: bounds.height / size.rows, left: bounds.left, top: bounds.top };
  };

  // xterm's WebGL renderer writes upright text's font with a leading space
  // (an empty style, then the weight). Chromium 150 refuses that and keeps
  // 10px sans-serif, which drew every terminal's text tiny (0.3.177); the
  // guard TerminalView installs reads past the space.
  const glyphs = document.createElement('canvas').getContext('2d')!;
  glyphs.font = `${''} ${400} ${12 * devicePixelRatio}px monospace`;
  assert(glyphs.font === `${12 * devicePixelRatio}px monospace`,
    `The canvas took xterm's upright font as ${glyphs.font}`);

  view.focus();
  // Claude Code never sends DECSCUSR, so the default cursor is what it shows.
  await write('$ prompt-studio ');
  await shown(true, 'Focused default cursor was not drawn by the caret');
  const grid = cell();
  // The default caret is already drawn at column 0 on focus, before the prompt paints.
  await until(() => Math.abs(caret()!.getBoundingClientRect().left - (grid.left + 16 * grid.width)) < 0.5,
    'Caret is not at the cursor column');
  const first = caret()!.getBoundingClientRect();
  assert(Math.abs(first.width - 2) <= 0.5 / devicePixelRatio, `Caret is ${first.width}px wide`);
  assert(Math.abs(first.height - grid.height * 0.85) <= 0.5 / devicePixelRatio,
    `Caret is ${first.height}px tall in a ${grid.height}px row`);
  assert(first.top > grid.top && first.bottom < grid.top + grid.height, 'Caret is not inside the cursor row');
  const palette = getComputedStyle(caret()!);
  const color = palette.backgroundColor;
  const expectedColor = palette.colorScheme === 'dark' ? 'rgb(130, 151, 232)' : 'rgb(82, 109, 204)';
  assert(color === expectedColor, `Caret blue is ${color}, expected ${expectedColor}`);

  await write('\r\n> ');
  await frames();
  const followed = (): boolean => {
    const moved = caret()!.getBoundingClientRect();
    return Math.abs(moved.left - (grid.left + 2 * grid.width)) < 0.5 && moved.top > grid.top + grid.height;
  };
  await until(followed, () => `Caret did not follow the cursor: ${JSON.stringify(caret()?.getBoundingClientRect())} ${JSON.stringify(grid)}`);

  await write('\x1b[?25l');
  await shown(false, 'Hidden cursor kept the caret');
  await write('\x1b[?25h');
  await shown(true, 'Shown cursor did not restore the caret');
  await write('\x1b[2 q');
  await shown(false, 'Block cursor kept the caret');
  // Codex sends CSI 0 SP q: back to the configured cursor.
  await write('\x1b[0 q');
  await shown(true, 'Default cursor after DECSCUSR 0 was not drawn by the caret');
  await write('\x1b[1 q');
  await shown(false, 'Blinking block kept the caret');
  await write('\x1b[5 q');
  await shown(true, 'Blinking bar was not drawn by the caret');
  host.querySelector('textarea')!.blur();
  await shown(false, 'Unfocused pane kept the caret instead of xterm\'s outline');
  view.focus();
  await shown(true, 'Refocused pane did not draw the caret');
  await write('\x1b[2 q');
  await shown(false, 'Block cursor kept the caret');
  await write('\x1b[!p');
  await shown(true, 'Soft reset did not return to the caret');
  // Leave the supplied prompt shape visible for the renderer capture.
  const rule = '─'.repeat(view.size()!.columns - 1);
  await write(`\x1b[2J\x1b[H\x1b[90m${rule}\r\n\x1b[0m❯ \r\n\x1b[90m${rule}\r\n\x1b[33m» auto mode on\x1b[0m\x1b[2;3H`);
  await frames();
  return { width: first.width, height: first.height, row: grid.height, color, hides: ['DECTCEM', 'block', 'blur'] };
}
