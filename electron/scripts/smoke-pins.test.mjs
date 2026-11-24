import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { PACKAGED_LAUNCH_PROFILE_IDS, RENDERER_PINS } from "./smoke-packaged.mjs";

// The packaged smoke is the only test that drives the real bundle, and it costs
// about thirteen minutes to learn that one selector was renamed. Every literal
// the probe asserts against the renderer is declared in RENDERER_PINS with the
// file that carries it; this test reads those files and fails in a second.
const repoRoot = new URL("../../", import.meta.url);
const rendererRoot = new URL("../src/renderer/", import.meta.url);

const pinUrl = pin => new URL(pin.file, pin.repo === true ? repoRoot : rendererRoot);
// What the file is actually searched for: the enclosing source text when the
// pin declares one, the bare literal otherwise.
const pinNeedle = pin => pin.context ?? pin.literal;

test("the packaged launcher roster matches the Rust owner's complete profile order", async () => {
  const source = await readFile(new URL("crates/interface-terminal/src/launch.rs", repoRoot), "utf8");
  const order = source.match(/const PROFILE_ORDER: \[TerminalLaunchProfileId; \d+\] = \[([\s\S]*?)\];/);
  assert.ok(order, "the Rust launch-profile roster could not be located");
  const variants = [...order[1].matchAll(/TerminalLaunchProfileId::(\w+)/g)].map(match => match[1]);
  assert.ok(variants.length > 0);
  const expected = variants.map(variant => {
    const pin = RENDERER_PINS.find(candidate => candidate.file === "crates/interface-contracts/src/terminal.rs"
      && candidate.context === `Self::${variant} => "${candidate.literal}"`);
    assert.ok(pin, `Rust profile ${variant} is missing from the packaged smoke pins`);
    return pin.literal;
  });
  assert.deepEqual(PACKAGED_LAUNCH_PROFILE_IDS, expected,
    "Update the packaged smoke roster when the host adds or removes launch profiles");
});

function occurrences(source, needle) {
  let count = 0;
  for (let at = source.indexOf(needle); at !== -1; at = source.indexOf(needle, at + 1)) {
    count += 1;
  }
  return count;
}

async function readPinnedSources() {
  const sources = new Map();
  for (const pin of RENDERER_PINS) {
    const url = pinUrl(pin);
    if (sources.has(url.href)) continue;
    sources.set(url.href, await readFile(url, "utf8").catch(error => {
      assert.fail(`pin "${pin.literal}" names ${pin.file}, which cannot be read: ${
        error instanceof Error ? error.message : String(error)}`);
    }));
  }
  return sources;
}

test("every renderer pin still exists verbatim in the file it names", async () => {
  assert.ok(RENDERER_PINS.length > 0, "RENDERER_PINS is empty");
  const sources = await readPinnedSources();
  for (const pin of RENDERER_PINS) {
    const needle = pinNeedle(pin);
    assert.ok(
      sources.get(pinUrl(pin).href).includes(needle),
      `pin "${pin.literal}" is gone from ${pin.file}${
        pin.context === undefined ? "" : ` (searched for ${JSON.stringify(needle)})`
      }. The packaged smoke probe still asserts it: update the renderer, or `
      + "update RENDERER_PINS in smoke-packaged.mjs.",
    );
  }
});

// A pin its file matches for some unrelated reason cannot fail on the rename it
// exists to catch, which is the whole contract. `terminal-pane` lives on inside
// `terminal-pane-head`; `Settings` is a word App.tsx uses forty other times.
test("a literal its file repeats is pinned through an enclosing context", async () => {
  const sources = await readPinnedSources();
  for (const pin of RENDERER_PINS) {
    const found = occurrences(sources.get(pinUrl(pin).href), pin.literal);
    if (pin.context === undefined) {
      assert.equal(found, 1,
        `pin "${pin.literal}" occurs ${String(found)} times in ${pin.file}, so matching it `
        + "cannot prove the renderer still names that surface. Give the pin a context: the "
        + "enclosing source text (an aria-label, a className, a CSS selector) to search for.");
      continue;
    }
    assert.ok(pin.context.includes(pin.literal),
      `pin "${pin.literal}" has a context that does not contain it (${
        JSON.stringify(pin.context)}); the probe would assert one string and this test another.`);
    assert.notEqual(pin.context, pin.literal,
      `pin "${pin.literal}" has a context identical to its literal; drop the context instead.`);
  }
});

test("each pin is declared once and names a path inside the project", () => {
  const seen = new Set();
  for (const pin of RENDERER_PINS) {
    assert.equal(typeof pin.literal, "string", `a pin has no literal: ${JSON.stringify(pin)}`);
    assert.notEqual(pin.literal, "", "a pin declares an empty literal");
    assert.equal(typeof pin.file, "string", `pin "${pin.literal}" has no file`);
    assert.ok(!pin.file.startsWith("/") && !pin.file.includes(".."),
      `pin "${pin.literal}" must name a project-relative path, not ${pin.file}`);
    const key = `${pin.file} | ${pin.literal}`;
    assert.ok(!seen.has(key), `pin "${pin.literal}" is declared twice for ${pin.file}`);
    seen.add(key);
  }
});

// The rename that broke the 2026-09-04 packaged run: the pane header's close
// button stopped being `[title="Close tab"]` and became a named
// close-active-terminal action. The pin below is the one that catches it.
test("the pane header's close action is pinned to the component that renders it", () => {
  const pin = RENDERER_PINS.find(
    candidate => candidate.literal === "Close active terminal (ends it)",
  );
  assert.ok(pin !== undefined, "the pane close action is not pinned");
  assert.equal(pin.file, "components/TerminalPane.tsx");
  assert.notEqual(pin.repo, true);
  assert.equal(pin.context, 'aria-label="Close active terminal (ends it)"');
});
