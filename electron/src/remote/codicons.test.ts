import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';

// The phone page loads the vendored codicon subset, so a glyph missing from it
// draws nothing (the reply bar's Claude sparkle did, until 2026-09-26).
it('names only codicons the vendored subset draws', () => {
  const css = readFileSync(join(__dirname, '../renderer/codicons.css'), 'utf8');
  const sources = readdirSync(__dirname).filter(file => /\.tsx?$/.test(file) && !file.includes('.test.'));
  const used = new Set<string>();
  for (const file of sources) {
    const text = readFileSync(join(__dirname, file), 'utf8');
    expect(text, `${file} builds a codicon class name at runtime`).not.toMatch(/codicon-\$\{/);
    for (const [, name] of text.matchAll(/codicon-([a-z0-9-]+)/g)) {
      if (!name.startsWith('modifier-')) used.add(name);
    }
  }
  expect(used.size).toBeGreaterThan(0);
  for (const name of used) expect(css, name).toContain(`.codicon-${name}::before`);
});
