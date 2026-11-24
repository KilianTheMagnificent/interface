import type { IBufferLine, Terminal } from '@xterm/xterm';

export type CursorProfile = 'codex' | 'claude';
interface Point { row: number; column: number }
interface PromptRow { row: number; stops: readonly number[] }
interface Prompt { rows: readonly PromptRow[]; cursor: Point; signature: string }

const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const border = /^\s*[─━-]{3,}\s*$/u;

/** Insertion boundaries are graphemes, not UTF-16 offsets or terminal cells. */
export function terminalCursorStops(line: IBufferLine, start: number, columns: number,
    cursor = start): number[] {
  let text = '';
  const cells: number[] = [];
  let end = start;
  for (let column = start; column < columns; column++) {
    const cell = line.getCell(column);
    if (!cell || cell.getWidth() === 0) continue;
    const chars = cell.isDim() ? ' ' : cell.getChars() || ' ';
    for (let index = 0; index < chars.length; index++) cells.push(column);
    text += chars;
    if (chars.trim() !== '' || column < cursor) end = column + cell.getWidth();
  }
  const stops = [...graphemes.segment(text)].map(part => cells[part.index])
    .filter(column => column < end);
  stops.push(end);
  return stops;
}

function background(line: IBufferLine | undefined, column: number): string {
  const cell = line?.getCell(column);
  return cell ? `${cell.getBgColorMode()}:${cell.getBgColor()}` : '';
}

/** Read only the live native input band. Output, menus and unknown layouts do
 * not acquire click-to-edit behavior. The CLI remains the draft/cursor owner. */
function promptAtCursor(terminal: Terminal, profile: CursorProfile | undefined): Prompt | null {
  const buffer = terminal.buffer.active;
  if (!profile || terminal.options.disableStdin || !terminal.modes.bracketedPasteMode
    || terminal.modes.mouseTrackingMode !== 'none' || buffer.viewportY !== buffer.baseY) return null;
  const cursor = { row: buffer.baseY + buffer.cursorY, column: buffer.cursorX };
  const bottom = buffer.baseY + terminal.rows;
  let first = cursor.row;
  let start = 0;
  for (; first >= Math.max(buffer.baseY, cursor.row - 50); first--) {
    const line = buffer.getLine(first);
    const match = (profile === 'codex' ? /^\s*› /u : /^\s*❯ /u)
      .exec(line?.translateToString(true) ?? '');
    if (match && !line?.isWrapped) { start = match[0].length; break; }
    if (border.test(line?.translateToString(true) ?? '')) return null;
  }
  if (!start || (!buffer.getLine(cursor.row)?.isWrapped && cursor.column < start)) return null;
  const firstLine = buffer.getLine(first)!;
  const fill = background(firstLine, start);
  const shaded = profile === 'codex' && firstLine.getCell(start)?.getBgColorMode() !== 0
    && [1, 2, 3].some(offset => fill !== background(buffer.getLine(first - offset), start));
  let last = first;
  let bounded = shaded;
  for (let row = first + 1; row < Math.min(bottom, first + 51); row++) {
    const line = buffer.getLine(row);
    const text = line?.translateToString(true) ?? '';
    if (border.test(text)) { bounded = true; break; }
    if (shaded && background(line, start) !== fill) break;
    // Codex also draws an unshaded prompt. Its status/footer is outside the
    // editable area and separated from it by a blank row.
    if (!shaded && profile === 'codex' && /^\s{2,}\S.*(?: · |for shortcuts|context left)/u.test(text)
      && (buffer.getLine(row - 1)?.translateToString(true).trim() ?? '') === '') {
      bounded = true; break;
    }
    if (text.trim() !== '' && !text.startsWith(' '.repeat(start)) && !line?.isWrapped) {
      if (!shaded && profile === 'codex') break;
      return null;
    }
    last = row;
  }
  // Unboxed prompts still support their current single line.
  if (!bounded) last = first;
  if (cursor.row > last) return null;
  const signature: string[] = [String(buffer.baseY), String(first), String(terminal.cols)];
  const rows: PromptRow[] = [];
  for (let row = first; row <= last; row++) {
    const line = buffer.getLine(row)!;
    const contentStart = row > first && line.isWrapped ? 0 : start;
    rows.push({ row, stops: terminalCursorStops(line, contentStart, terminal.cols,
      row === cursor.row ? cursor.column : contentStart) });
    signature.push(line.translateToString(false), background(line, start));
  }
  // Padding below the native draft is not another editable line. Keep empty
  // lines inside a multiline draft, including the actual cursor's empty row.
  while (rows.length > 1 && rows.at(-1)!.row > cursor.row && rows.at(-1)!.stops.length === 1) rows.pop();
  for (let row = Math.max(buffer.baseY, first - 6); row < bottom; row++) {
    if (row >= first && row <= last) continue;
    const text = buffer.getLine(row)?.translateToString(true) ?? '';
    if (/esc to (?:cancel|interrupt)|enter to (?:confirm|select)|allow once|approve once/iu.test(text)
      || /^\s*[›❯]?\s*\d+[.)]\s/u.test(text)) return null;
  }
  if (/^\s*[›❯]\s*\d+[.)]\s/u.test(firstLine.translateToString(true))) return null;
  return { rows, cursor, signature: signature.join('\n') };
}

function nearest(stops: readonly number[], column: number): number {
  let best = 0;
  for (let index = 1; index < stops.length; index++) {
    if (Math.abs(stops[index] - column) < Math.abs(stops[best] - column)) best = index;
  }
  return best;
}

/** Cross one visual row at a time, then observe the CLI's real cursor before
 * continuing. This handles both soft wraps and explicit newlines without
 * reconstructing the draft or guessing how many invisible separators it has.
 * Ctrl-B/F are native character movement; no Escape can interrupt a provider. */
function nextMovement(prompt: Prompt, target: Point): string | null {
  const current = prompt.rows.find(row => row.row === prompt.cursor.row);
  const destination = prompt.rows.find(row => row.row === target.row);
  if (!current || !destination) return null;
  const at = nearest(current.stops, prompt.cursor.column);
  const delta = target.row < current.row ? -at - 1
    : target.row > current.row ? current.stops.length - at
    : nearest(current.stops, target.column) - at;
  return delta === 0 ? null : (delta < 0 ? '\x02' : '\x06').repeat(Math.min(512, Math.abs(delta)));
}

/** One disposable set of native gesture listeners per emulator generation. */
export function installTerminalCursorPlacement(terminal: Terminal,
    state: () => { profile: CursorProfile | undefined; enabled: boolean }): () => void {
  const host = terminal.element!;
  const screen = host.querySelector<HTMLElement>('.xterm-screen')!;
  const document = host.ownerDocument;
  let down: { x: number; y: number; time: number; prompt: Prompt } | null = null;
  let pending: { target: Point; signature: string; previous: Point; expires: number } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let sending = false;
  let disposed = false;
  const cancel = (): void => { pending = null; down = null; clearTimeout(timer); timer = undefined; };
  const read = (): Prompt | null => {
    if (disposed) return null;
    const current = state();
    return current.enabled && !terminal.hasSelection() ? promptAtCursor(terminal, current.profile) : null;
  };
  const send = (prompt: Prompt): void => {
    if (!pending) return;
    const input = nextMovement(prompt, pending.target);
    if (!input) { cancel(); return; }
    pending.previous = prompt.cursor;
    sending = true;
    try { terminal.input(input, true); } finally { sending = false; }
  };
  const plain = (event: MouseEvent): boolean => event.button === 0 && event.detail === 1
    && !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey;
  const inScreen = (event: MouseEvent): boolean => event.target instanceof Node
    && screen.contains(event.target) && !screen.classList.contains('xterm-cursor-pointer')
    && !(event.target instanceof Element && event.target.closest('a'));
  const mouseDown = (event: MouseEvent): void => {
    cancel();
    if (!plain(event) || !inScreen(event)) return;
    const prompt = read();
    if (prompt) down = { x: event.clientX, y: event.clientY, time: performance.now(), prompt };
  };
  const mouseMove = (event: MouseEvent): void => {
    if (down && Math.hypot(event.clientX - down.x, event.clientY - down.y) > 4) down = null;
  };
  const mouseUp = (event: MouseEvent): void => {
    const pressed = down;
    down = null;
    if (!pressed || !plain(event) || !inScreen(event) || performance.now() - pressed.time > 500
      || Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) > 4) return;
    // Let xterm finish its native selection gesture first.
    queueMicrotask(() => {
      const prompt = read();
      if (!prompt || prompt.signature !== pressed.prompt.signature
        || prompt.cursor.row !== pressed.prompt.cursor.row
        || prompt.cursor.column !== pressed.prompt.cursor.column) return;
      const bounds = screen.getBoundingClientRect();
      if (!bounds.width || !bounds.height || event.clientX < bounds.left || event.clientX >= bounds.right
        || event.clientY < bounds.top || event.clientY >= bounds.bottom) return;
      const target = {
        row: terminal.buffer.active.viewportY + Math.floor((event.clientY - bounds.top) * terminal.rows / bounds.height),
        column: (event.clientX - bounds.left) * terminal.cols / bounds.width,
      };
      if (!prompt.rows.some(row => row.row === target.row)) return;
      pending = { target, signature: prompt.signature, previous: prompt.cursor, expires: performance.now() + 2000 };
      terminal.focus();
      send(prompt);
    });
  };
  const writes = terminal.onWriteParsed(() => {
    if (!pending || timer !== undefined) return;
    timer = setTimeout(() => {
      timer = undefined;
      const prompt = read();
      if (!pending || !prompt || performance.now() > pending.expires || prompt.signature !== pending.signature) {
        cancel(); return;
      }
      if (prompt.cursor.row === pending.previous.row && prompt.cursor.column === pending.previous.column) {
        return;
      }
      send(prompt);
    }, 32);
  });
  const input = terminal.onData(() => { if (!sending) cancel(); });
  const resize = terminal.onResize(cancel);
  host.addEventListener('mousedown', mouseDown, true);
  document.addEventListener('mousemove', mouseMove);
  document.addEventListener('mouseup', mouseUp);
  for (const event of ['keydown', 'compositionstart', 'paste', 'wheel', 'blur']) host.addEventListener(event, cancel, true);
  return () => {
    disposed = true;
    cancel();
    writes.dispose(); input.dispose(); resize.dispose();
    host.removeEventListener('mousedown', mouseDown, true);
    document.removeEventListener('mousemove', mouseMove);
    document.removeEventListener('mouseup', mouseUp);
    for (const event of ['keydown', 'compositionstart', 'paste', 'wheel', 'blur']) host.removeEventListener(event, cancel, true);
  };
}
