import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import lanes from "./renderer-lanes.cjs";

const { LANES, ONLY, selectLanes } = lanes;
const names = Object.keys(LANES);
const gate = names.filter(name => LANES[name].gate !== false);

test("the gate runs every gate lane, in the listed order", () => {
  assert.deepEqual(selectLanes({}), gate);
  assert.ok(gate.includes("default") && !gate.includes("pane-swap"));
});

test("a lane list selects those lanes and the lanes whose reports they read", () => {
  assert.deepEqual(selectLanes({ INTERFACE_RENDERER_LANE: "keyboard-input, away" }), ["away", "keyboard-input"]);
  assert.deepEqual(selectLanes({ INTERFACE_RENDERER_LANE: "default" }),
    names.filter(name => name === "default" || LANES.default.after.includes(name)));
  assert.throws(() => selectLanes({ INTERFACE_RENDERER_LANE: "away,missing" }), /Unknown renderer lane missing/);
});

test("the one-lane switches keep working, the first one set winning", () => {
  assert.deepEqual(selectLanes({ INTERFACE_TERMINAL_CARET_ONLY: "1" }), ["terminal-caret-1x", "terminal-caret-2x"]);
  assert.deepEqual(selectLanes({ INTERFACE_WORKSPACE_HIERARCHY_ONLY: "1" }),
    ["workspace-hierarchy-dark", "workspace-hierarchy-light"]);
  assert.deepEqual(selectLanes({ INTERFACE_AGENT_OVERVIEW_ONLY: "1", INTERFACE_AWAY_ONLY: "1" }), ["away"]);
  assert.deepEqual(selectLanes({ INTERFACE_AWAY_ONLY: "1", INTERFACE_RENDERER_LANE: "pane-menu" }), ["pane-menu"]);
  for (const [, selected] of ONLY) assert.ok(selected.every(name => names.includes(name)));
});

test("every lane has a bounded budget, earlier dependencies and an implementation", async () => {
  const main = await readFile(new URL("./renderer-fixture-main.cjs", import.meta.url), "utf8");
  const run = main.slice(main.indexOf("const RUN = {"));
  for (const [index, name] of names.entries()) {
    const { seconds, after = [] } = LANES[name];
    assert.ok(Number.isInteger(seconds) && seconds >= 60 && seconds <= 180, `${name}: ${seconds} s`);
    for (const dependency of after) assert.ok(names.indexOf(dependency) > -1 && names.indexOf(dependency) < index, `${name} after ${dependency}`);
    assert.match(run, new RegExp(`\\n  (${name}|"${name}"):`), `${name} has no RUN entry`);
  }
});
