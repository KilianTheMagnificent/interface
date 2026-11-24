import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import { TerminalView, type TerminalViewHandle } from '../components/TerminalView';
import type { CursorProfile } from '../terminal-cursor';
import { assert, untilWithin, wait } from './fixture-support';

const until = untilWithin({ pollMs: 10 });

/** The fixture owns a tiny native editor. The product sees only rendered PTY
 * bytes and must navigate its actual cursor, including invisible newlines. */
export async function terminalCursorFixture(root: Root): Promise<Record<string, unknown>> {
  const engines: CursorProfile[] = ['codex', 'claude'];
  const handles = new Map<number, TerminalViewHandle>();
  const segment = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  const drafts = engines.map(() => [...segment.segment('alpha 界é beta gamma delta epsilon zeta eta theta iota kappa lambda\nsecond line')].map(s => s.segment));
  const cursors = drafts.map(draft => draft.length);
  const inputs: Array<{ index: number; data: string }> = [];
  let working = false;
  let shadedCodex = true;
  let writes = 0;
  const firstRow = 4;
  const width = (text: string): number => text === '界' ? 2 : 1;
  const layout = (index: number): { lines: string[]; points: Array<{ row: number; column: number }> } => {
    const columns = handles.get(index)!.size()!.columns;
    const lines = [''];
    const points = [{ row: firstRow, column: 2 }];
    let row = 0;
    let column = 2;
    for (const char of drafts[index]) {
      if (char === '\n') { lines.push(''); row++; column = 2; }
      else {
        if (column + width(char) > columns - 2) { lines.push(''); row++; column = 2; points[points.length - 1] = { row: firstRow + row, column }; }
        lines[row] += char; column += width(char);
      }
      points.push({ row: firstRow + row, column });
    }
    return { lines, points };
  };
  const draw = (index: number): void => {
    const handle = handles.get(index)!;
    const { lines, points } = layout(index);
    const columns = handle.size()!.columns;
    const codex = engines[index] === 'codex';
    let output = '\x1b[0m\x1b[2J\x1b[HOutput remains selectable\x1b[?2004h';
    output += `\x1b[${firstRow};1H` + (codex ? (shadedCodex ? '\x1b[48;5;236m' : '') + ' '.repeat(columns) : '─'.repeat(columns));
    lines.forEach((line, at) => {
      output += `\x1b[${firstRow + at + 1};1H` + (at === 0 ? codex ? '› ' : '❯ ' : '  ') + line;
      output += ' '.repeat(Math.max(0, columns - 2 - [...segment.segment(line)].reduce((sum, p) => sum + width(p.segment), 0)));
    });
    output += `\x1b[${firstRow + lines.length + 1};1H` + (codex ? ' '.repeat(columns) : '─'.repeat(columns));
    output += `\x1b[0m\x1b[${firstRow + lines.length + 2};1H  gpt-6-astra max fast · /fixture`;
    const point = points[cursors[index]];
    output += `\x1b[${point.row + 1};${point.column + 1}H`;
    writes++;
    handle.write(output, () => { writes--; });
  };
  const receive = (index: number, data: string): void => {
    inputs.push({ index, data });
    for (const char of data) {
      if (char === '\x02') cursors[index] = Math.max(0, cursors[index] - 1);
      else if (char === '\x06') cursors[index] = Math.min(drafts[index].length, cursors[index] + 1);
      else if (char === 'x') { drafts[index].splice(cursors[index], 0, char); cursors[index]++; }
      else throw new Error(`Unexpected cursor input: ${JSON.stringify(data)}`);
    }
    draw(index);
  };
  const render = (): void => flushSync(() => root.render(<div style={{ display: 'flex', height: 400 }}>
    {engines.map((engine, index) => <div key={engine} style={{ width: 420, height: 400 }}>
      <TerminalView terminalId={`cursor-${engine}`} active cursorProfile={engine} working={working}
        onData={data => receive(index, data)} onReady={handle => {
          if (handle) { handles.set(index, handle); handle.completeSetup(); }
        }} />
    </div>)}
  </div>));
  render();
  await until(() => handles.size === 2, 'Cursor fixtures did not mount');
  for (let index = 0; index < engines.length; index++) {
    if (engines[index] === 'claude') handles.get(index)!.write('\x1b[?1049h');
    draw(index);
  }
  await until(() => writes === 0, 'Initial cursor fixtures did not render');
  const screen = (index: number): HTMLElement => document.querySelector<HTMLElement>(`[data-terminal-id="cursor-${engines[index]}"] .xterm-screen`)!;
  const coords = (index: number, point: { row: number; column: number }): { clientX: number; clientY: number } => {
    const bounds = screen(index).getBoundingClientRect();
    const size = handles.get(index)!.size()!;
    return { clientX: bounds.left + (point.column + 0.1) * bounds.width / size.columns,
      clientY: bounds.top + (point.row + 0.5) * bounds.height / size.rows };
  };
  const gesture = (index: number, point: { row: number; column: number }, extra: MouseEventInit = {}): void => {
    const options = { bubbles: true, cancelable: true, button: 0, detail: 1, ...coords(index, point), ...extra };
    screen(index).dispatchEvent(new MouseEvent('mousedown', { ...options, buttons: 1 }));
    screen(index).dispatchEvent(new MouseEvent('mouseup', options));
  };
  const click = async (index: number, target: number): Promise<void> => {
    gesture(index, layout(index).points[target]);
    // Built at the deadline, so it reports where the cursor ended up.
    await until(() => cursors[index] === target && writes === 0,
      () => `${engines[index]} click missed ${target}: cursor=${cursors[index]}; writes=${writes}; inputs=${JSON.stringify(inputs.slice(-5))}`);
    await wait(45);
  };
  // xterm sizes its screen once it has measured a cell, which can land after
  // the first writes drain. Until then the screen has no area: a click aimed
  // by its bounds lands at its corner and is rightly refused.
  await until(() => engines.every((_, index) => {
    const bounds = screen(index).getBoundingClientRect();
    return bounds.width > 0 && bounds.height > 0;
  }), 'Cursor fixtures never laid out their screens');
  let placements = 0;
  for (let index = 0; index < engines.length; index++) {
    // Native idle TUIs can keep repainting decorations. Cursor acknowledgments
    // must make progress even when output never goes quiet for a whole frame.
    const animation = setInterval(() => handles.get(index)!.write('\x1b7\x1b[2;1H.\x1b8'), 8);
    try {
      for (const target of [6, 9, drafts[index].length - 3, 0, 40, drafts[index].length]) {
        await click(index, target); placements++;
      }
    } finally { clearInterval(animation); }
    if (index === 0) {
      shadedCodex = false; draw(index);
      await until(() => writes === 0, 'Unshaded Codex prompt did not render');
      for (const target of [4, drafts[index].length - 2, 42, 0]) {
        await click(index, target); placements++;
      }
    }
    await click(index, 7); placements++;
    const before = drafts[index].join('');
    const field = screen(index).closest('.terminal-view')!.querySelector('textarea')!;
    field.dispatchEvent(new KeyboardEvent('keypress', { key: 'x', charCode: 120, keyCode: 120, bubbles: true, cancelable: true }));
    await until(() => writes === 0, 'Typed edit did not render');
    assert(drafts[index].join('') === before.replace('界', '界x'), 'Typed edit missed the clicked Unicode boundary');
    const prior = inputs.length;
    gesture(index, { row: 0, column: 5 });
    await wait(60);
    assert(inputs.length === prior, 'Clicking output injected input');
    gesture(index, layout(index).points[2], { ctrlKey: true });
    await wait(60);
    assert(inputs.length === prior, 'Modified click acquired cursor behavior');
    working = true; render();
    gesture(index, layout(index).points[2]);
    await wait(60);
    assert(inputs.length === prior, 'Busy provider received cursor input');
    working = false; render();
    // A real keystroke owns the cursor immediately, even between the row hops
    // of an earlier click. The pending click must never resume after that edit.
    cursors[index] = drafts[index].length; draw(index);
    await until(() => writes === 0, 'Reset did not render');
    gesture(index, layout(index).points[0]);
    await Promise.resolve();
    field.dispatchEvent(new KeyboardEvent('keypress', { key: 'x', charCode: 120, keyCode: 120, bubbles: true, cancelable: true }));
    const afterTyping = cursors[index];
    await wait(100);
    assert(cursors[index] === afterTyping, 'Old click moved the cursor after a newer keystroke');
    const beforeDrag = inputs.length;
    // A drag leaves xterm selection intact and never becomes a cursor move.
    const from = coords(index, layout(index).points[1]);
    const to = coords(index, layout(index).points[5]);
    screen(index).dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0, buttons: 1, detail: 1, ...from }));
    screen(index).dispatchEvent(new MouseEvent('mousemove', { bubbles: true, buttons: 1, ...to }));
    screen(index).dispatchEvent(new MouseEvent('mouseup', { bubbles: true, button: 0, detail: 1, ...to }));
    await wait(60);
    assert(inputs.length === beforeDrag, 'Selection drag emitted cursor input');
    assert(handles.get(index)!.getSelection().length > 0, 'Click handling broke native text selection');
  }
  assert(inputs.every(input => !input.data.includes('\x1b')), 'Cursor placement emitted Escape');
  return { profiles: engines, codexLayouts: ['shaded', 'unshaded'], placements, typedEdits: 4, typingCancelsPendingClick: true, unicode: 'wide and combining',
    multiline: 'soft wraps and explicit newlines', outputClicks: 'no input', selections: 'retained', busy: 'no input' };
}
