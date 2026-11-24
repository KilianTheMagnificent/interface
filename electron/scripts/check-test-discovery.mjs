import { readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

// Compare the runner's actual discovery with supported test files on disk.
// A custom include must never silently omit a future TSX component test.
const expected = readdirSync('src', { recursive: true })
  .filter(path => /\.test\.tsx?$/.test(path)).map(path => `src/${path}`);
const result = spawnSync(process.execPath, ['node_modules/vitest/vitest.mjs', 'list', '--filesOnly'], {
  encoding: 'utf8', timeout: 30_000,
});
if (result.error || result.status !== 0) {
  throw new Error(`Test discovery failed: ${result.error?.message ?? result.stderr}`);
}
const discovered = new Set(result.stdout.trim().split(/\r?\n/));
const missing = expected.filter(path => !discovered.has(path));
if (missing.length) throw new Error(`Tests outside Vitest discovery:\n${missing.join('\n')}`);
console.log(`Test discovery includes all ${expected.length} TS/TSX files.`);
