import { describe, expect, it } from 'vitest';
import { terminalPromptInputReady, terminalControlScreen, type TerminalPromptBuffer } from './terminal-prompt';

function input(lines: string[], cursorY: number, cursorX = 2, dim = false): TerminalPromptBuffer {
  return { baseY: 0, viewportY: 0, cursorY, cursorX, length: lines.length,
    getLine: row => lines[row] === undefined ? undefined : { isWrapped: false,
      translateToString: () => lines[row], getCell: column => ({
        getChars: () => lines[row][column] ?? '', isDim: () => dim ? 1 : 0,
      }) } };
}

describe('Native terminal prompt guard', () => {
  it.each([5, 75, 146])('keeps the prompt and footer in bounded settings observations in a tall window at row %i', cursorY => {
    const lines = Array.from({ length: 150 }, (_, row) => `screen row ${row}`);
    lines[cursorY] = '› Ask Codex to do anything';
    lines[cursorY + 2] = '  gpt-6-astra max · /repo';
    const screen = terminalControlScreen(input(lines, cursorY, 2, true), 150, true)!;
    expect(screen.lines).toHaveLength(100);
    expect(screen.lines[screen.cursorRow]).toBe(lines[cursorY]);
    expect(screen.lines[screen.cursorRow + 2]).toBe(lines[cursorY + 2]);
    expect(screen.cursorColumn).toBe(2);
    expect(screen.ghostText).toBe(true);
    expect(screen.atBottom).toBe(true);
    const draft = terminalControlScreen(input(lines, cursorY), 150, true)!;
    expect(draft.ghostText).toBe(false);
    expect(terminalControlScreen({ ...input(lines, cursorY), viewportY: 1 }, 150, true)?.atBottom).toBe(false);
  });
  it('accepts empty Claude and Codex prompts with bracketed paste', () => {
    expect(terminalPromptInputReady(input(['❯ '], 0), 'claude', true)).toBe(true);
    expect(terminalPromptInputReady(input(['› '], 0), 'codex', true)).toBe(true);
    expect(terminalPromptInputReady(input(['› '], 0), 'codex', false)).toBe(false);
    expect(terminalPromptInputReady(input(['1. Completed the review', '2. Checked the result', '', '› '], 3), 'codex', true)).toBe(true);
  });
  it('distinguishes a dimmed suggestion from a native draft with its cursor at the start', () => {
    expect(terminalPromptInputReady(input(['› Implement a feature'], 0, 2, true), 'codex', true)).toBe(true);
    expect(terminalPromptInputReady(input(['› existing draft'], 0), 'codex', true)).toBe(false);
    expect(terminalPromptInputReady(input(['❯ existing draft'], 0, 16), 'claude', true)).toBe(false);
  });
  it('recognizes Codex prompt particles only beside a dim hint and with their native cell styles', () => {
    const text = '›⠁Ask Codex to do anything   ⠈';
    const particles = new Set([1, text.length - 1]);
    const buffer = input([text], 0);
    const decorated: TerminalPromptBuffer = { ...buffer, getLine: row => ({ ...buffer.getLine(row)!,
      getCell: column => ({ getChars: () => text[column] ?? '',
        isDim: () => column >= 2 && !particles.has(column) ? 1 : 0,
        getFgColorMode: () => particles.has(column) ? 0x3000000 : 0,
        getFgColor: () => 0x656565, getBgColorMode: () => 0x3000000,
      }),
    }) };
    expect(terminalPromptInputReady(decorated, 'codex', true)).toBe(true);
    expect(terminalControlScreen(decorated, 1, true)).toMatchObject({
      cursorRow: 0, cursorColumn: 2, ghostText: true, lines: ['› Ask Codex to do anything   ⠈'],
    });
    // A user-authored Braille draft has neither a dim hint at the cursor nor
    // the particle style. Even explicitly colored draft text stays protected.
    expect(terminalPromptInputReady(input(['› ⠁'], 0), 'codex', true)).toBe(false);
    const draft = { ...decorated, getLine: (row: number) => ({ ...decorated.getLine(row)!,
      translateToString: () => '› ⠁', getCell: (column: number) => ({
        ...decorated.getLine(row)!.getCell!(column)!, getChars: () => '› ⠁'[column] ?? '', isDim: () => 0,
      }),
    }) };
    expect(terminalPromptInputReady(draft, 'codex', true)).toBe(false);
    expect(terminalControlScreen(draft, 1, true)?.ghostText).toBe(false);
    expect(terminalPromptInputReady({ ...decorated, viewportY: 1 }, 'codex', true)).toBe(false);
  });
  it('refuses selection and approval screens even when they share a prompt glyph', () => {
    expect(terminalPromptInputReady(input(['❯ 1. Yes', '  2. No'], 0, 2, true), 'claude', true)).toBe(false);
    expect(terminalPromptInputReady(input(['Approve once or always', '› '], 1), 'codex', true)).toBe(false);
    expect(terminalPromptInputReady(input(['❯ ', 'Enter to select'], 0), 'claude', true)).toBe(false);
    // A printed yes/no question directly above the prompt draws no list, and
    // remote input must not write into that prompt.
    expect(terminalPromptInputReady(input(['Do you want to overwrite it? (y/n)', '› '], 1), 'codex', true)).toBe(false);
    // The assistant's own sentence is not a CLI holding a keystroke, and it
    // leaves the composer exactly as available as it was before the card.
    expect(terminalPromptInputReady(input(['Want me to overwrite it? (y/n)', '› '], 1), 'codex', true)).toBe(true);
    expect(terminalPromptInputReady(input(['Do you want to overwrite it?', 'Say the word.', '› '], 2), 'codex', true)).toBe(true);
  });
  it('refuses scrollback, shell prompts, wrapped input and a prompt above the cursor', () => {
    const buffer = input(['› previous prompt', '', 'answer'], 2);
    expect(terminalPromptInputReady(buffer, 'codex', true)).toBe(false);
    expect(terminalPromptInputReady(input(['$ '], 0), 'codex', true)).toBe(false);
    expect(terminalPromptInputReady({ ...input(['› '], 0), viewportY: 1 }, 'codex', true)).toBe(false);
    const blank = input(['› '], 0);
    expect(terminalPromptInputReady({ ...blank, getLine: row => ({ ...blank.getLine(row)!, isWrapped: true }) }, 'codex', true)).toBe(false);
  });
});
