import type { TerminalControlScreenV1 } from '../shared/ipc';

const APPROVAL_WORDING =
  /would you like to|do you want to|select an option|permission (?:required|request)/iu;
/** A printed yes/no key: the whole answer on a screen that offers no list. */
const CONFIRM_KEY = /[[(]\s*y(?:es)?\s*\/\s*n(?:o)?\s*[\])]/iu;

export interface TerminalPromptBuffer {
  readonly baseY: number;
  readonly viewportY: number;
  readonly cursorY: number;
  readonly length: number;
  readonly cursorX: number;
  getLine(row: number): {
    readonly isWrapped: boolean;
    translateToString(trimRight: boolean): string;
    getCell?(column: number): {
      getChars(): string;
      isDim(): number;
      getFgColorMode?(): number;
      getFgColor?(): number;
      getBgColorMode?(): number;
    } | undefined;
  } | undefined;
}

type InputLine = NonNullable<ReturnType<TerminalPromptBuffer['getLine']>>;
/** Codex paints single-dot particles over the blank prompt's shaded band.
 * They use a neutral RGB foreground and RGB background, unlike typed text.
 * Keep this a cell-style observation: unstyled Braille in a draft is text. */
function promptDecoration(line: InputLine | undefined, column: number, cursorColumn: number): boolean {
  const hint = line?.getCell?.(cursorColumn);
  if (!hint?.isDim() || !/\p{L}/u.test(hint.getChars())) return false;
  const cell = line?.getCell?.(column);
  if (!cell || !/^[⠁⠂⠄⠈⠐⠠⡀⢀]$/u.test(cell.getChars())
    || cell.getFgColorMode?.() !== 0x3000000 || cell.getBgColorMode?.() !== 0x3000000) return false;
  const color = cell.getFgColor?.() ?? -1;
  if (color < 0) return false;
  const channels = [(color >> 16) & 255, (color >> 8) & 255, color & 255];
  return Math.max(...channels) - Math.min(...channels) <= 3;
}

function promptText(line: InputLine | undefined, cursorColumn: number): string {
  const text = line?.translateToString(false) ?? '';
  // A particle can occupy the SPACE after ›. Preserve the glyph, cursor and
  // every ordinary character while translating that decorated gap to a space.
  if (/^\s*›/u.test(text)) {
    return text.slice(0, cursorColumn).split('').map((char, column) =>
      promptDecoration(line, column, cursorColumn) ? ' ' : char).join('') + text.slice(cursorColumn);
  }
  return text;
}

/** Frozen DTO translation only; Rust recognizes prompts, menus and settings. */
export function terminalControlScreen(buffer: TerminalPromptBuffer, rows: number,
    bracketedPaste: boolean): TerminalControlScreenV1 | null {
  if (rows < 1 || buffer.cursorY < 0 || buffer.cursorY >= rows) return null;
  // The wire accepts 100 rows, but a large window can fit more. Keep the
  // cursor, the context above it and the native footer inside a bounded slice
  // instead of dropping every settings observation in that window.
  const firstRow = Math.max(0, Math.min(buffer.cursorY - 12, rows - 100));
  const lines = Array.from({ length: Math.min(rows, 100) }, (_, row) =>
    buffer.getLine(buffer.baseY + firstRow + row)?.translateToString(true) ?? '');
  const cursorRow = buffer.cursorY - firstRow;
  const cursor = buffer.getLine(buffer.baseY + buffer.cursorY);
  lines[cursorRow] = promptText(cursor, buffer.cursorX).trimEnd();
  let ghostText = cursor?.getCell !== undefined;
  for (let column = buffer.cursorX; column < (lines[cursorRow]?.length ?? 0); column++) {
    const cell = cursor?.getCell?.(column);
    if (cell?.getChars().trim() && !cell.isDim()
      && !(/^\s*›/u.test(lines[cursorRow]) && promptDecoration(cursor, column, buffer.cursorX))) ghostText = false;
  }
  return { lines, cursorRow, cursorColumn: buffer.cursorX,
    atBottom: buffer.viewportY === buffer.baseY, bracketedPaste, ghostText };
}

/** A conservative visual input guard. Only an empty prompt at the actual
 * cursor is eligible; native drafts, menus and unknown layouts stay native.
 * Ghost suggestions after the cursor are allowed, just as in the CLI. */
export function terminalPromptInputReady(buffer: TerminalPromptBuffer,
    engine: 'claude' | 'codex', bracketedPaste: boolean): boolean {
  if (!bracketedPaste || buffer.viewportY !== buffer.baseY) return false;
  const row = buffer.baseY + buffer.cursorY;
  const line = buffer.getLine(row);
  if (!line || line.isWrapped) return false;
  const text = promptText(line, buffer.cursorX);
  const match = (engine === 'claude' ? /^\s*❯\s/u : /^\s*› /u).exec(text);
  if (!match || buffer.cursorX !== match[0].length) return false;
  // Cursor-at-start is also possible in an existing draft. Only dimmed ghost
  // suggestions may follow it; never append a GUI message to that draft.
  if (text.slice(buffer.cursorX).trim() !== '') {
    if (!line.getCell) return false;
    for (let column = buffer.cursorX; column < text.length; column++) {
      const cell = line.getCell(column);
      if (cell?.getChars().trim() && !cell.isDim()
        && !(engine === 'codex' && promptDecoration(line, column, buffer.cursorX))) return false;
    }
  }
  // Approval and selection UIs can also put a chevron at column zero.
  let confirmAbove = false;
  let requestWording = false;
  for (let i = Math.max(buffer.baseY, row - 6); i < buffer.length; i++) {
    const content = buffer.getLine(i)?.translateToString(true) ?? '';
    if ((i >= row && /^\s*[›❯]?\s*\d+[.)]\s/u.test(content))
      || /(?:allow|approve|permission).*(?:once|always|deny)|esc to cancel|enter to (?:confirm|select)/iu.test(content)) return false;
    // A printed yes/no question directly above the prompt is a CLI holding one
    // keystroke, even though it has drawn no list. Remote input must not write
    // into that prompt. Require both the key and the phrasing of a request.
    if (i === row - 1 && CONFIRM_KEY.test(content)) confirmAbove = true;
    if (APPROVAL_WORDING.test(content)) requestWording = true;
  }
  return !(confirmAbove && requestWording);
}
