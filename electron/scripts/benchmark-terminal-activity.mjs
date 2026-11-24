// Local parser microbenchmark, not a whole-app latency or CPU measurement.
// Run from electron/: node scripts/benchmark-terminal-activity.mjs [baseline-revision]
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { transformWithOxc } from 'vite';

const root = fileURLToPath(new URL('../..', import.meta.url));
const baseline = process.argv[2];
if (process.argv.length > 3) throw new Error('Expected at most one baseline revision.');
const revision = baseline ? execFileSync('git', ['rev-parse', '--verify', '--end-of-options',
  `${baseline}^{commit}`], { cwd: root, encoding: 'utf8' }).trim() : null;
const sources = ['terminal-output-state', 'terminal-activity'];
const encoder = new TextEncoder();

async function benchmark(ref) {
  const directory = await mkdtemp(path.join(tmpdir(), 'interface-activity-bench-'));
  try {
    for (const name of sources) {
      const filename = `electron/src/renderer/${name}.ts`;
      const source = ref ? execFileSync('git', ['show', `${ref}:${filename}`],
        { cwd: root, encoding: 'utf8' }) : await readFile(path.join(root, filename), 'utf8');
      const { code } = await transformWithOxc(source, filename);
      await writeFile(path.join(directory, `${name}.mjs`),
        code.replace(/(["'])\.\/terminal-output-state\1/g, '"./terminal-output-state.mjs"'));
    }
    const { createTerminalActivityMonitor, feedTerminalActivity } = await import(
      pathToFileURL(path.join(directory, 'terminal-activity.mjs')).href);
    const results = [];
    for (const size of [4096, 16384, 65536]) {
      for (const [pattern, text] of [['text', 'plain output '], ['ansi', 'x\x1b[0m'], ['bell', 'x\x07']]) {
        const bytes = encoder.encode(text.repeat(Math.ceil(size / text.length))).subarray(0, size);
        const timings = [];
        for (let trial = 0; trial < 25; trial++) {
          const monitor = createTerminalActivityMonitor();
          const start = performance.now();
          feedTerminalActivity(monitor, bytes);
          const elapsed = performance.now() - start;
          if (trial >= 5) timings.push(elapsed);
        }
        timings.sort((a, b) => a - b);
        const result = { source: ref ?? 'working-tree', pattern, bytes: size,
          medianMs: (timings[9] + timings[10]) / 2 };
        results.push(result);
        process.stdout.write(`${JSON.stringify(result)}\n`);
      }
    }
    return results;
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

process.stdout.write(`${JSON.stringify({ node: process.version, warmup: 5, measured: 20,
  measurement: 'one fresh monitor and one feed; monitor construction excluded' })}\n`);
if (revision) await benchmark(revision);
await benchmark(null);
