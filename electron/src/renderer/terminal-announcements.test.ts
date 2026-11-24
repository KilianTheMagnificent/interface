import { describe, expect, it } from 'vitest';
import { boundedTerminalAnnouncement, TERMINAL_ANNOUNCEMENT_LIMIT as LIMIT,
  TERMINAL_ANNOUNCEMENT_NOTICE as NOTICE } from './terminal-announcements';

describe('terminal announcements', () => {
  it('preserves ordinary announcements and xterm clear boundaries', () => {
    for (const text of ['', 'Approval needed: continue?\n', 'x'.repeat(LIMIT)]) {
      expect(boundedTerminalAnnouncement(text)).toBe(text);
    }
  });
  it('retains recent output with one notice, including repeated animation appends', () => {
    let text = '';
    for (let frame = 0; frame < 1_000; frame++) {
      text = boundedTerminalAnnouncement(text + '⠁⠈ ⢀⠐ ⠄⠠ ⡀⠂ '.repeat(8));
      expect(text.length).toBeLessThanOrEqual(LIMIT);
    }
    // Once truncated, strip the existing notice before retaining new output.
    expect(text.split(NOTICE)).toHaveLength(2);
    expect(text.endsWith('⠁⠈ ⢀⠐ ⠄⠠ ⡀⠂ ')).toBe(true);
    expect(boundedTerminalAnnouncement(text)).toBe(text);
  });
  it('does not split a Unicode surrogate pair at the truncation boundary', () => {
    const suffix = 'x'.repeat(LIMIT - NOTICE.length - 1);
    const text = boundedTerminalAnnouncement('prefix'.repeat(LIMIT) + '😀' + suffix);
    expect(text).toBe(NOTICE + suffix);
    expect(text.length).toBeLessThanOrEqual(LIMIT);
  });
});
