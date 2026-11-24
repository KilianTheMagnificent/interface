"use strict";
// The isolated renderer DOM lanes behind `npm run test:renderer`. Each lane is
// its own Electron process with its own profile, launched by
// scripts/test-renderer-dom.mjs and implemented in the RUN table of
// scripts/renderer-fixture-main.cjs.
//
// `seconds` bounds one lane process from launch to exit (boot, module load and
// every fixture). It is sized to the lane's own work: about five times its time
// alone on an idle machine, and never under a minute, so a loaded gate keeps
// headroom while a hang still fails in bounded time. Idle times measured on
// 27 Sep 2026 are noted; a lane that grows past a fifth of its budget should
// be split, as interactions and workspace-hierarchy were. The pool also starts
// the lanes with the most budget ahead of them first.
// `after` names lanes whose JSON artifacts this lane reads: it starts once they
// passed, and selecting it selects them. `gate: false` lanes repeat part of a
// gate lane and run only when selected.
const LANES = {
  customization: { seconds: 60 },
  away: { seconds: 60 }, // 1 s
  "pane-motion": { seconds: 60 }, // 4 s
  interactions: { seconds: 180 }, // 39 s
  "interactions-agents": { seconds: 120 }, // 20 s
  "shell-renders": { seconds: 60 }, // 2 s
  "themes-refined-1100": { seconds: 60 }, // 2 s
  "themes-refined-640": { seconds: 60 }, // 2 s
  "themes-cli-1100": { seconds: 60 }, // 2 s
  "themes-cli-640": { seconds: 60 }, // 3 s
  default: { seconds: 90, after: ["interactions", "interactions-agents", // 16 s
    "themes-refined-1100", "themes-refined-640", "themes-cli-1100", "themes-cli-640"] },
  // 10 s idle, but it has overrun a minute on a loaded machine.
  "working-indicator": { seconds: 90 },
  "workspace-toolbar": { seconds: 60 }, // 11 s
  "sidebar-peek": { seconds: 60 },
  "pane-menu": { seconds: 60 }, // 3 s
  "workspace-hierarchy-dark": { seconds: 150 }, // 30 s
  "workspace-hierarchy-light": { seconds: 150 }, // 30 s
  "workspace-launcher": { seconds: 75 }, // 13 s
  "terminal-interaction": { seconds: 120 }, // 25 s
  "terminal-reopen": { seconds: 60 }, // 5 s
  "terminal-caret-1x": { seconds: 60 }, // 1 s
  "terminal-caret-2x": { seconds: 60 }, // 1 s
  "keyboard-input": { seconds: 60 }, // 1 s
  "agent-overview": { seconds: 60 }, // 19 s
  "output-preview": { seconds: 60 }, // 3 s
  "browser-point": { seconds: 90 },
  "browser-chrome": { seconds: 120 },
  "visual-reply": { seconds: 90 },
  threads: { seconds: 60 },
  "commit-selection": { seconds: 60 },
  workflows: { seconds: 60 },
  "pane-swap": { seconds: 60, gate: false }, // within interactions
  "sidebar-reference": { seconds: 60, gate: false }, // within interactions
  "thread-inbox": { seconds: 60 },
  "terminal-preservation": { seconds: 90, gate: false }, // within terminal-interaction
};

// The older one-lane switches, checked in this order: the first one set wins.
const ONLY = [
  ["INTERFACE_AWAY_ONLY", ["away"]],
  ["INTERFACE_PANE_MOTION_ONLY", ["pane-motion"]],
  ["INTERFACE_PANE_SWAP_ONLY", ["pane-swap"]],
  ["INTERFACE_KEYBOARD_INPUT_ONLY", ["keyboard-input"]],
  ["INTERFACE_TERMINAL_INTERACTION_ONLY", ["terminal-interaction"]],
  ["INTERFACE_TERMINAL_REOPEN_ONLY", ["terminal-reopen"]],
  ["INTERFACE_TERMINAL_CARET_ONLY", ["terminal-caret-1x", "terminal-caret-2x"]],
  ["INTERFACE_TERMINAL_PRESERVATION_ONLY", ["terminal-preservation"]],
  ["INTERFACE_WORKSPACE_LAUNCHER_ONLY", ["workspace-launcher"]],
  ["INTERFACE_WORKSPACE_HIERARCHY_ONLY", ["workspace-hierarchy-dark", "workspace-hierarchy-light"]],
  ["INTERFACE_PANE_MENU_ONLY", ["pane-menu"]],
  ["INTERFACE_WORKSPACE_TOOLBAR_ONLY", ["workspace-toolbar"]],
  ["INTERFACE_SIDEBAR_REFERENCE_ONLY", ["sidebar-reference"]],
  ["INTERFACE_THREAD_INBOX_ONLY", ["thread-inbox"]],
  ["INTERFACE_WORKING_INDICATOR_ONLY", ["working-indicator"]],
  ["INTERFACE_OUTPUT_PREVIEW_ONLY", ["output-preview"]],
  ["INTERFACE_BROWSER_POINT_ONLY", ["browser-point"]],
  ["INTERFACE_BROWSER_CHROME_ONLY", ["browser-chrome"]],
  ["INTERFACE_THREADS_ONLY", ["threads"]],
  ["INTERFACE_SHELL_RENDERS_ONLY", ["shell-renders"]],
  ["INTERFACE_AGENT_OVERVIEW_ONLY", ["agent-overview"]],
];

/** The lanes to run, in serial order: `INTERFACE_RENDERER_LANE=a,b`, else the
 * first `INTERFACE_*_ONLY` switch set, else every gate lane. */
function selectLanes(env) {
  const listed = (env.INTERFACE_RENDERER_LANE ?? "").split(",").map(name => name.trim()).filter(Boolean);
  const chosen = listed.length ? listed
    : ONLY.find(([key]) => env[key])?.[1]
      ?? Object.keys(LANES).filter(name => LANES[name].gate !== false);
  const unknown = chosen.filter(name => !Object.hasOwn(LANES, name));
  if (unknown.length) {
    throw new Error(`Unknown renderer lane ${unknown.join(", ")}. Lanes: ${Object.keys(LANES).join(", ")}`);
  }
  const wanted = new Set();
  const add = name => {
    if (wanted.has(name)) return;
    wanted.add(name);
    for (const dependency of LANES[name].after ?? []) add(dependency);
  };
  chosen.forEach(add);
  return Object.keys(LANES).filter(name => wanted.has(name));
}

/** A lane's budget plus the longest chain of lanes waiting on it. */
function laneRank(name) {
  const waiting = Object.keys(LANES).filter(other => (LANES[other].after ?? []).includes(name));
  return LANES[name].seconds + Math.max(0, ...waiting.map(laneRank));
}

module.exports = { LANES, ONLY, selectLanes, laneRank };
