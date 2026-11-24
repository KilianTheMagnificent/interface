const { app, BrowserWindow, MessageChannelMain, ipcMain } = require("electron");
const { PassThrough } = require("node:stream");
const { randomUUID } = require("node:crypto");
const { pathToFileURL } = require("node:url");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { LANES } = require("./renderer-lanes.cjs");
const [url, output, lane] = process.argv.slice(2);

// Any throw, including one inside an Electron event handler, fails the lane at
// once. Without a handler Electron raises a modal error box that blocks the
// main thread, and with it the lane's own deadline.
const fail = error => { console.error(error); app.exit(1); };
process.on("uncaughtException", fail);
process.on("unhandledRejection", fail);
if (!Object.hasOwn(LANES, lane)) {
  console.error(`Unknown renderer lane ${lane}`);
  process.exit(1);
}
setTimeout(() => {
  console.error(`Renderer fixture deadline exceeded: lane ${lane} ran past ${LANES[lane].seconds} s`);
  app.exit(1);
}, LANES[lane].seconds * 1000).unref();

// One Chromium profile per lane: concurrent lanes never share storage or locks.
app.setPath("userData", path.join(output, `profile-${lane}`));
// The Browser lane grants a camera request: Chromium's fake devices, so no
// real camera or microphone is ever opened.
if (lane === "browser-chrome") app.commandLine.appendSwitch("use-fake-device-for-media-stream");
// No Dock icon and no activation: the gate runs while the owner works.
if (process.platform === "darwin") app.setActivationPolicy("accessory");

const js = (window, code) => window.webContents.executeJavaScript(code);
/** Calls a fixture; a rejection carries the renderer's stack. */
const fixture = (window, call) => js(window, `Promise.resolve(${call}).catch(error => { throw new Error(error?.stack ?? String(error)); })`);
const shot = async (window, name, rect) =>
  fs.writeFile(path.join(output, `${name}.png`), (await window.webContents.capturePage(rect)).toPNG());
const media = (window, features) => window.webContents.debugger.sendCommand("Emulation.setEmulatedMedia", { features });
const motion = value => ({ name: "prefers-reduced-motion", value });
const THEMES = ["dark", "light"];
const CHROMES = ["refined", "cli"];

/** Sizes the window, runs one fixture call and keeps its screenshot, per case. */
async function matrix(window, cases) {
  const reports = [];
  for (const { width, height, call, name } of cases) {
    window.setContentSize(width, height);
    reports.push(await fixture(window, call));
    if (name) await shot(window, name);
  }
  return reports;
}

/** Hidden Chromium can hand back the preceding frame: capture until the
 * pixels show the current palette, for up to three seconds. */
async function captureWhenPainted(window, name, failure) {
  const deadline = Date.now() + 3000;
  for (;;) {
    window.webContents.invalidate();
    const image = await window.webContents.capturePage();
    const capture = await js(window, `window.terminalCaptureMatches(${JSON.stringify(image.toDataURL())})`);
    if (capture.matches) return fs.writeFile(path.join(output, `${name}.png`), image.toPNG());
    assert.ok(Date.now() < deadline, `${failure}: ${JSON.stringify(capture.samples)}`);
    await new Promise(resolve => setTimeout(resolve, 20));
  }
}

async function exerciseSidebarReference(window) {
  const reports = [];
  for (const theme of THEMES) for (const chrome of CHROMES) {
    for (const width of [288, 240]) {
      try {
        reports.push(await js(window, `window.sidebarReferenceFixture('${theme}', '${chrome}', ${width})`));
      } finally {
        await shot(window, `sidebar-${theme}-${chrome}-${width}`, { x: 0, y: 0, width, height: 660 });
      }
    }
  }
  await media(window, [motion("no-preference")]);
  reports.push(await js(window, "window.sidebarReferenceFixture('dark', 'refined', 288)"));
  await media(window, [motion("reduce"), { name: "prefers-reduced-transparency", value: "no-preference" }]);
  return reports;
}

// The hidden sidebar's hover reveal, driven by the native pointer: real
// pointerover/out/enter and button state reach the hook as they do in the app.
async function exerciseSidebarPeek(window) {
  window.setContentSize(1280, 800);
  await js(window, "window.sidebarPeekFixture()");
  const read = () => js(window, "window.sidebarPeekState()");
  const pause = ms => js(window, `new Promise(resolve => setTimeout(resolve, ${ms}))`);
  const mouse = (type, x, y, pressed = false) => window.webContents.debugger.sendCommand("Input.dispatchMouseEvent",
    { type, x, y, button: pressed ? "left" : "none", buttons: pressed && type !== "mouseReleased" ? 1 : 0,
      clickCount: type === "mouseMoved" ? 0 : 1 });
  const click = async (x, y) => {
    await mouse("mouseMoved", x, y);
    await mouse("mousePressed", x, y, true);
    await mouse("mouseReleased", x, y, true);
  };
  // Longer than the open delay and the close grace, with room for a busy machine.
  const settle = () => pause(700);
  const report = {};
  await mouse("mouseMoved", 700, 400); await settle();
  const hidden = await read();
  assert.equal(hidden.visibility, "hidden", "The hidden sidebar is reachable before the toggle is touched");
  // Just right of the traffic lights, over the column it brings back.
  assert.ok(hidden.toggle.left > hidden.traffic.right && hidden.toggle.left - hidden.traffic.right <= 8
    && hidden.toggle.left >= hidden.inset && hidden.toggle.right <= hidden.inset + hidden.paneLeft,
    `The sidebar toggle is not beside the traffic lights, over the sidebar: ${JSON.stringify([hidden.traffic, hidden.toggle])}`);
  const toggle = { x: Math.round((hidden.toggle.left + hidden.toggle.right) / 2),
    y: Math.round((hidden.toggle.top + hidden.toggle.bottom) / 2) };

  // The window's left edge no longer answers (owner, 2026-10-20).
  await mouse("mouseMoved", 2, 400); await settle();
  assert.ok(!(await read()).peek, "Resting on the left edge still brought the sidebar back");

  await mouse("mouseMoved", toggle.x, toggle.y); await settle();
  const shown = report.shown = await read();
  assert.ok(shown.peek && shown.visibility === "visible", "Resting on the toggle did not bring the sidebar back");
  assert.ok(Math.abs(shown.sidebar.left - shown.inset) <= 1 && Math.abs(shown.sidebar.right - shown.inset - shown.paneLeft) <= 1,
    `The revealed sidebar is not where its column puts it: ${JSON.stringify(shown.sidebar)}`);
  assert.ok(shown.sidebar.top >= shown.toggle.bottom, "The revealed sidebar covers its toggle");
  assert.equal(shown.shellLeft, hidden.shellLeft, "Revealing the sidebar moved the work under it");
  assert.ok(shown.onTop, "The revealed sidebar is under the work");
  await shot(window, "sidebar-peek-shown");

  // Straight down off the toggle is inside the panel.
  await mouse("mouseMoved", toggle.x, Math.round(shown.toggle.bottom) + 4);
  await mouse("mouseMoved", toggle.x, 200); await settle();
  assert.ok((await read()).peek, "The sidebar went away on the way down from its toggle");
  await mouse("mouseMoved", 700, 400); await settle();
  const left = await read();
  assert.ok(!left.peek && left.visibility === "hidden", "The sidebar stayed after the pointer left it");

  // A drag passing over the toggle never summons it.
  await mouse("mousePressed", 700, 400, true);
  await mouse("mouseMoved", 300, 200, true);
  await mouse("mouseMoved", toggle.x, toggle.y, true); await settle();
  assert.ok(!(await read()).peek, "A drag over the toggle brought the sidebar back");
  await mouse("mouseMoved", 700, 400, true);
  await mouse("mouseReleased", 700, 400, true);
  await mouse("mouseMoved", toggle.x, toggle.y); await settle();
  assert.ok((await read()).peek, "The toggle did not answer once the button was released");

  // A click keeps it, from where it already is.
  await click(toggle.x, toggle.y);
  await mouse("mouseMoved", 700, 400); await settle();
  const pinned = report.pinned = await read();
  assert.ok(pinned.docked && pinned.visibility === "visible", "The rail toggle did not keep the revealed sidebar");
  assert.ok(Math.abs(pinned.sidebar.left - shown.sidebar.left) <= 1, "Keeping the revealed sidebar moved it");

  // Hiding it leaves the pointer resting on the toggle: that alone, or moving
  // on it, must not bring it straight back; leaving and returning does.
  await click(toggle.x, toggle.y); await settle();
  await mouse("mouseMoved", toggle.x + 2, toggle.y + 1); await settle();
  const stayed = await read();
  assert.ok(!stayed.docked && !stayed.peek, "Hiding the sidebar from its toggle brought it straight back");
  await mouse("mouseMoved", 700, 400);
  await mouse("mouseMoved", toggle.x, toggle.y); await settle();
  assert.ok((await read()).peek, "The toggle did not answer after the pointer came back");
  await mouse("mouseMoved", 700, 400); await settle();

  // With motion on: it slides in, and the toggle keeps it without a jump
  // while its column opens underneath.
  await media(window, [motion("no-preference")]);
  try {
    await mouse("mouseMoved", toggle.x, toggle.y);
    await pause(150);
    const arriving = report.arriving = await js(window, "window.sidebarPeekTrack(6)");
    assert.ok(arriving.some(left => left < shown.sidebar.left - 1) && arriving.at(-1) > arriving[0],
      `The sidebar did not slide in: ${arriving.join(", ")}`);
    await settle();
    await click(toggle.x, toggle.y);
    const keeping = report.keeping = await js(window, "window.sidebarPeekTrack(12)");
    assert.ok(keeping.every(left => Math.abs(left - shown.sidebar.left) <= 1.5),
      `Keeping the revealed sidebar made it jump: ${keeping.join(", ")}`);
  } finally {
    await media(window, [motion("reduce"), { name: "prefers-reduced-transparency", value: "no-preference" }]);
  }
  return { sidebarPeek: report };
}

async function capturePaneSwap(window) {
  for (const [theme, chrome, width] of [["dark", "refined", 1100], ["light", "refined", 640], ["dark", "cli", 640]]) {
    window.setSize(width, 760);
    await js(window, `document.documentElement.dataset.theme = '${theme}'; document.documentElement.dataset.chrome = '${chrome}'; window.terminalPaneSwapFixture(true)`);
    await shot(window, `pane-swap-${theme}-${chrome}-${width}`);
  }
}

// Every lane: what it runs, and the report it prints. Budgets and pool order
// live in renderer-lanes.cjs.
const RUN = {
  customization: async window => {
    window.setContentSize(960, 760);
    const result = await fixture(window, "window.customizationFixture()");
    await shot(window, "customization");
    return { customization: result };
  },
  away: async window => ({ away: await matrix(window, [1100, 390].map(width =>
    ({ width, height: 860, call: "window.awayFixture()", name: `away-${width}` }))) }),
  "shell-renders": async window => ({ shellRenders: await fixture(window, "window.shellRenderFixture()") }),
  "terminal-reopen": async window => ({
    terminalReopen: await fixture(window, "window.terminalReopenFixture()"),
    terminalExitHandoff: await fixture(window, "window.terminalExitHandoffFixture()"),
    terminalHandoffSwitch: await fixture(window, "window.terminalHandoffSwitchFixture()"),
  }),
  "terminal-interaction": async window => ({ terminalInteraction: await fixture(window, "window.terminalInteractionFixture()") }),
  "terminal-preservation": async window => ({ terminalPreservation: await fixture(window, "window.terminalPreservationFixture()") }),
  "terminal-caret-1x": window => terminalCaret(window, 1),
  "terminal-caret-2x": window => terminalCaret(window, 2),
  "keyboard-input": async window => ({ keyboardInput: await require("./keyboard-input-fixture.cjs").exerciseKeyboardInput(window) }),
  // Point and fix against a real page in a real <webview> (one of the two lanes that allow one).
  "browser-point": async window => ({ browserPoint: await require("./browser-point-lane.cjs").exerciseBrowserPoint(window, output) }),
  // A Browser tab that behaves like a Chrome tab, against real pages in real <webview>s.
  "browser-chrome": async window => ({ browserChrome: await require("./browser-chrome-lane.cjs").exerciseBrowserChrome(window, output) }),
  "visual-reply": async window => ({ visualReply: await require("./visual-reply-lane.cjs").exerciseVisualReply(window, output) }),
  "workspace-launcher": async window => {
    const cases = [];
    for (const theme of THEMES) for (const chrome of CHROMES) for (const width of [1100, 640, 360]) {
      cases.push({ width, height: 900, call: `window.workspaceLauncherFixture('${theme}', '${chrome}')`, name: `launcher-${theme}-${chrome}-${width}` });
    }
    return { workspaceLauncher: await matrix(window, cases) };
  },
  // One lane per theme: the whole matrix took most of a minute in one process.
  "workspace-hierarchy-dark": window => workspaceHierarchy(window, "dark"),
  "workspace-hierarchy-light": window => workspaceHierarchy(window, "light"),
  "output-preview": async window => {
    const reports = [];
    for (const [theme, chrome, width] of [["dark", "refined", 1440], ["light", "refined", 640], ["dark", "cli", 390]]) {
      window.setContentSize(width, 860);
      const name = `output-preview-${theme}-${chrome}-${width}`;
      try {
        reports.push(await fixture(window, `window.outputPreviewFixture('${theme}', '${chrome}')`));
      } finally {
        await shot(window, name);
        console.log(`Output preview capture: ${output}/${name}.png`);
      }
      await js(window, "window.clearOutputPreviewFixture()");
    }
    return { outputPreview: reports };
  },
  // The New thread card in each mode, a worktree thread's pane with its
  // diff panel docked (1280) and floating over the panes (900, CLI chrome),
  // and Settings → Threads.
  "commit-selection": async window => {
    window.setContentSize(640, 860);
    const result = await fixture(window, "window.commitSelectionFixture()");
    await shot(window, "commit-selection");
    return result;
  },
  workflows: async window => {
    const cases = THEMES.map(theme => ({ width: 900, height: 900, name: `workflows-${theme}`,
      call: `window.workflowsFixture('${theme}')` }));
    return { workflows: await matrix(window, cases) };
  },
  threads: async window => {
    const cases = [];
    for (const theme of THEMES) {
      for (const mode of ["none", "local", "worktree"]) {
        cases.push({ width: 1100, height: 760, name: `thread-card-${mode}-${theme}`,
          call: `window.threadCardFixture('${theme}', 'refined', '${mode}')` });
      }
      cases.push({ width: 1280, height: 800, name: `thread-pane-${theme}`,
        call: `window.threadPaneFixture('${theme}', 'refined')` });
    }
    cases.push({ width: 900, height: 760, name: "thread-pane-dark-cli-900",
      call: "window.threadPaneFixture('dark', 'cli')" });
    // A measurement: 300 finished turns opened, streamed into and scrolled.
    cases.push({ width: 1100, height: 860, name: "thread-long-transcript-dark",
      call: "window.threadLongTranscriptFixture('dark', 'refined')" });
    cases.push({ width: 1100, height: 760, name: "thread-settings-dark",
      call: "window.threadSettingsFixture('dark', 'refined')" });
    // The Threads composer pressed: at rest with its + menu open, mid-turn,
    // and in a narrow CLI pane where its row has to wrap.
    cases.push({ width: 1100, height: 760, name: "thread-composer-dark-menu",
      call: "window.threadComposerFixture('dark', 'refined', 'menu')" });
    cases.push({ width: 1100, height: 760, name: "thread-composer-light-busy",
      call: "window.threadComposerFixture('light', 'refined', 'busy')" });
    cases.push({ width: 420, height: 760, name: "thread-composer-dark-cli-420",
      call: "window.threadComposerFixture('dark', 'cli', 'busy')" });
    // The live turn (owner, 2026-10-07): running, waiting, steered, finished
    // and a sent prompt, reduced here; with motion on below.
    cases.push({ width: 1100, height: 760, name: "thread-live-running-dark",
      call: "window.threadLiveFixture('dark', 'refined', 'running')" });
    cases.push({ width: 1100, height: 760, name: "thread-live-waiting-light",
      call: "window.threadLiveFixture('light', 'refined', 'waiting')" });
    cases.push({ width: 1100, height: 760, name: "thread-live-dark",
      call: "window.threadLiveFixture('dark', 'refined')" });
    // The live turn: a request docked above the box, the box and the head in
    // amber, two follow-ups queued (owner, 2026-10-07).
    cases.push({ width: 1100, height: 760, name: "thread-composer-dark-waiting",
      call: "window.threadComposerFixture('dark', 'refined', 'waiting')" });
    cases.push({ width: 1100, height: 760, name: "thread-composer-light-waiting",
      call: "window.threadComposerFixture('light', 'refined', 'waiting')" });
    // Review B4, B5 and E: second opinions above the composer, the limit
    // offer, the model chip's headroom, a continued thread's head, a
    // previewed Markdown file beside the thread, and the pane ⋯'s offers.
    for (const theme of THEMES) {
      cases.push({ width: 1440, height: 860, name: `thread-opinion-${theme}`,
        call: `window.threadOpinionFixture('${theme}', 'refined')` });
    }
    cases.push({ width: 1440, height: 860, name: "thread-opinion-menu-dark",
      call: "window.threadOpinionFixture('dark', 'refined', 'menu')" });
    // T3 section 9: quote a reply's selection, stash and restore a draft with
    // its image chip, and add a thread chip, in both palettes.
    for (const theme of THEMES) {
      cases.push({ width: 1100, height: 760, name: `composer-references-${theme}`,
        call: `window.composerReferencesFixture('${theme}')` });
    }
    // A hidden window's first capture can be the frame before the first
    // paint, so one is taken and dropped before the matrix.
    window.setContentSize(1100, 760);
    await fixture(window, "window.threadCardFixture('dark', 'refined', 'none')");
    window.webContents.invalidate();
    await window.webContents.capturePage();
    const threads = await matrix(window, cases);
    // The live turn's motion: shimmer, the turning ring, rows rising, each
    // beat of the finish and the sent prompt's rise, in both palettes.
    await media(window, [motion("no-preference")]);
    for (const theme of THEMES) threads.push(await fixture(window, `window.threadLiveFixture('${theme}', 'refined')`));
    // Once with motion on: the comet turns, the head's line runs and the
    // docked card arrives (the matrix above runs reduced, as every lane does).
    window.setContentSize(1100, 760);
    threads.push(await fixture(window, "window.threadComposerFixture('dark', 'refined', 'waiting')"));
    await media(window, [motion("reduce"), { name: "prefers-reduced-transparency", value: "no-preference" }]);
    await js(window, "window.clearThreadFixture()");
    await js(window, "window.clearThreadComposerFixture()");
    await js(window, "window.clearThreadOpinionFixture()");
    await js(window, "window.clearComposerReferencesFixture()");
    return { threads };
  },
  "agent-overview": async window => {
    const reports = [];
    const run = async (theme, chrome, width, name, tab = "agents") => {
      reports.push(...await matrix(window, [{ width, height: 760, call: `window.agentOverviewFixture('${theme}', '${chrome}', '${tab}')`, name }]));
      await js(window, "window.clearAgentOverviewFixture()");
    };
    // Docked beside the panes, and floating over them at 1080px and below.
    await run("dark", "refined", 1440, "agent-overview-dark-refined-1440");
    await run("light", "refined", 900, "agent-overview-light-refined-900");
    await run("dark", "cli", 900, "agent-overview-dark-cli-900");
    // Left on its Browser tab: the column's Browser, docked and floating.
    await run("dark", "refined", 1440, "agent-overview-browser-dark-refined-1440", "browser");
    await run("light", "refined", 900, "agent-overview-browser-light-refined-900", "browser");
    // Passes with motion on, so the pulse and the docked slide and floating
    // glide of the overview itself are exercised.
    await media(window, [motion("no-preference")]);
    await run("light", "refined", 1440, "agent-overview-light-refined-1440");
    await run("dark", "refined", 900);
    return { agentOverview: reports };
  },
  "workspace-toolbar": async window => {
    const cases = [];
    for (const theme of THEMES) for (const chrome of CHROMES) for (const width of [1280, 760, 640]) {
      cases.push({ width, height: 180, call: `window.workspaceToolbarFixture('${theme}', '${chrome}')`, name: `toolbar-${theme}-${chrome}-${width}` });
    }
    const toolbar = await matrix(window, cases);
    // The main area's corner notices (owner, 2026-11-15), in every palette form.
    const notices = [];
    for (const theme of THEMES) for (const chrome of CHROMES) {
      window.setContentSize(1000, 600);
      try {
        notices.push(await fixture(window, `window.cornerNoticesFixture('${theme}', '${chrome}')`));
      } finally {
        await shot(window, `corner-notices-${theme}-${chrome}`);
      }
    }
    return { toolbar, notices };
  },
  "sidebar-reference": async window => ({ sidebarReference: await exerciseSidebarReference(window) }),
  "sidebar-peek": window => exerciseSidebarPeek(window),
  "thread-inbox": async window => {
    window.setContentSize(1100, 760);
    const reports = [];
    for (const theme of THEMES) for (const chrome of CHROMES) {
      try {
        reports.push(await fixture(window, `window.threadInboxFixture('${theme}', '${chrome}', 288)`));
      } finally {
        await shot(window, `thread-inbox-${theme}-${chrome}`, { x: 0, y: 0, width: 288, height: 720 });
      }
    }
    // With motion on: the step's shimmer, the Needs you rings, the unread
    // dot's pop, the live line's fade and the float's slide.
    await media(window, [motion("no-preference")]);
    reports.push(await fixture(window, "window.threadInboxFixture('dark', 'refined', 288)"));
    await media(window, [motion("reduce"), { name: "prefers-reduced-transparency", value: "no-preference" }]);
    return { threadInbox: reports };
  },
  "working-indicator": async window => {
    const cases = [];
    for (const theme of await js(window, "window.themeNames")) for (const chrome of CHROMES) for (const width of [1100, 640]) {
      cases.push({ width, height: 860, call: `window.workingIndicatorFixture('${theme}', '${chrome}')`,
        name: THEMES.includes(theme) ? `wave-${theme}-${chrome}-${width}` : null });
    }
    const results = await matrix(window, cases);
    await media(window, [motion("no-preference")]);
    for (const engine of ["codex", "claude", "grok", "qwen", "opencode"]) for (const chrome of CHROMES) {
      results.push(await fixture(window, `window.workingIndicatorFixture('dark', '${chrome}', '${engine}')`));
    }
    return { workingIndicator: results };
  },
  "pane-menu": async window => ({ paneMenu: await matrix(window, [[1100, 860], [640, 560]].map(([width, height]) =>
    ({ width, height, call: "window.terminalPaneMenuFixture()", name: `pane-menu-${width}` }))) }),
  "pane-motion": paneMotion,
  "pane-swap": async window => {
    const paneSwap = await js(window, "window.terminalPaneSwapFixture()");
    await capturePaneSwap(window);
    return { paneSwap };
  },
  interactions: async window => {
    const report = await js(window, "window.runRendererFixtures()");
    for (const theme of THEMES) for (const width of [1100, 640]) {
      window.setContentSize(width, 760);
      await js(window, `document.documentElement.dataset.theme = '${theme}'; window.harnessVisual()`);
      await shot(window, `harness-${theme}-${width}`);
    }
    report.terminalPort = await exerciseTerminalPort(window);
    report.sidebarReference = await exerciseSidebarReference(window);
    await capturePaneSwap(window);
    await fs.writeFile(path.join(output, "interactions.json"), JSON.stringify(report, null, 2));
    return report;
  },
  "interactions-agents": async window => {
    const report = await js(window, "window.runAgentFixtures()");
    await fs.writeFile(path.join(output, "interactions-agents.json"), JSON.stringify(report, null, 2));
    return report;
  },
  // Each palette/viewport lane covers the complete theme matrix for one
  // chrome and width.
  "themes-refined-1100": window => themeMatrix(window, "refined", 1100),
  "themes-refined-640": window => themeMatrix(window, "refined", 640),
  "themes-cli-1100": window => themeMatrix(window, "cli", 1100),
  "themes-cli-640": window => themeMatrix(window, "cli", 640),
  default: defaultReport,
};

async function terminalCaret(window, scale) {
  const caretLooks = [];
  window.webContents.setZoomFactor(1);
  const nativeScale = await js(window, "devicePixelRatio");
  const zoom = scale / nativeScale;
  // Browser zoom scales CSS and the GPU drawable together. CDP's mobile
  // device emulation leaves WebGL's native backing scale unchanged on macOS.
  window.webContents.setZoomFactor(zoom);
  // The zoom reaches the page separately from executeJavaScript, and on a busy
  // machine it can land after the next read: wait for it rather than race it.
  const deadline = Date.now() + 3000;
  for (let ratio = await js(window, "devicePixelRatio"); ratio !== scale; ratio = await js(window, "devicePixelRatio")) {
    assert.ok(Date.now() < deadline, `devicePixelRatio stayed ${ratio} after zooming to ${scale}x`);
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  for (const theme of THEMES) {
    const caret = await js(window, `document.documentElement.dataset.theme = '${theme}'; window.terminalCaretFixture()`);
    caretLooks.push({ theme, scale, ...caret });
    await shot(window, `caret-${theme}-${scale}x`, { x: 0, y: 0, width: Math.round(440 * zoom), height: Math.round(260 * zoom) });
  }
  window.webContents.setZoomFactor(1);
  return { caretLooks };
}

async function workspaceHierarchy(window, theme) {
  const cases = [];
  // Six panes only at the packaged smoke's 1920px width: the grid that probe
  // used to pin by number is checked here, against the tile tokens.
  for (const chrome of CHROMES) for (const width of [2560, 1920, 1280, 760])
    for (const count of width === 1920 ? [4, 6, 8] : [4, 8]) {
    cases.push({ width, height: 900, call: `window.workspaceHierarchyFixture('${theme}', '${chrome}', ${count})`, name: `workspace-${theme}-${chrome}-${width}-${count}` });
  }
  const reports = await matrix(window, cases);
  if (theme === "dark") {
    // One pass with motion on, so the sidebar slide itself is exercised.
    window.setContentSize(1280, 900);
    await media(window, [motion("no-preference")]);
    reports.push(await fixture(window, "window.workspaceHierarchyFixture('dark', 'refined', 4, true)"));
  }
  return { workspaceHierarchy: reports };
}

async function paneMotion(window) {
  const reports = [];
  for (const reduced of ["no-preference", "reduce"]) for (const width of [1100, 640]) {
    window.setContentSize(width, 760);
    await media(window, [motion(reduced)]);
    reports.push(await js(window, "window.terminalPaneMotionFixture()"));
  }
  await media(window, [motion("no-preference")]);
  for (const [theme, width] of [["dark", 1100], ["light", 640]]) {
    window.setContentSize(width, 760);
    await js(window, `document.documentElement.dataset.theme = '${theme}'; window.terminalPaneMotionFixture(true)`);
    // Inspect the actual canvas pixels while motion is paused, not just buffer text.
    const textImage = await window.webContents.capturePage({ x: 12, y: 70, width: 200, height: 170 });
    const pixels = textImage.toBitmap();
    let ink = 0;
    for (let i = 0; i < pixels.length; i += 4) {
      if (Math.max(Math.abs(pixels[i] - pixels[0]), Math.abs(pixels[i + 1] - pixels[1]),
        Math.abs(pixels[i + 2] - pixels[2])) > 64) ink++;
    }
    assert(ink > 40, `Terminal canvas is blank during ${theme} pane motion`);
    await shot(window, `pane-motion-${theme}-${width}`);
    await js(window, `document.getAnimations().filter(a=>a.id==='terminal-pane-motion').forEach(a=>a.finish()); new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))`);
    await shot(window, `pane-motion-settled-${theme}-${width}`);
  }
  await fs.writeFile(path.join(output, "pane-motion.json"), JSON.stringify(reports, null, 2));
  return { paneMotion: reports };
}

async function themeMatrix(window, chrome, width) {
  const themes = await js(window, "window.themeNames");
  const visuals = {}; let structure;
  for (const theme of themes) {
    window.setContentSize(width, 760);
    const name = `${theme}-${chrome}-${width}`;
    visuals[name] = await js(window, `window.screenshotFixture(${JSON.stringify(theme)}, ${JSON.stringify(chrome)})`);
    if (theme === "dark") structure = visuals[name].structure;
    else assert.deepEqual(visuals[name].structure, structure, `${name} changed Dark's component structure`);
    await shot(window, name);
  }
  await fs.writeFile(path.join(output, `${lane}.json`), JSON.stringify(visuals));
  return { lane, themes: themes.length };
}

/** Joins the interactions and theme reports, then adds the terminal palette,
 * agent visuals, CLI pane appearance, translucency and material checks. */
async function defaultReport(window) {
  const themes = await js(window, "window.themeNames");
  const report = {};
  for (const part of ["interactions", "interactions-agents"]) {
    Object.assign(report, JSON.parse(await fs.readFile(path.join(output, `${part}.json`), "utf8")));
  }
  report.visuals = {};
  for (const chrome of CHROMES) for (const width of [1100, 640]) {
    Object.assign(report.visuals, JSON.parse(await fs.readFile(path.join(output, `themes-${chrome}-${width}.json`), "utf8")));
  }
  report.terminalThemes = {};
  window.setContentSize(1100, 760);
  for (const theme of themes) {
    const state = await js(window, `window.terminalThemeFixture(${JSON.stringify(theme)})`);
    if (theme !== "dark") assert.deepEqual(state.structure, report.terminalThemes.dark.structure, `${theme} changed terminal structure`);
    report.terminalThemes[theme] = state;
    await captureWhenPainted(window, `terminal-${theme}`, `${theme}: screenshot did not paint the current palette`);
  }
  for (const theme of ["light", "dark"]) {
    for (const width of [1100, 640]) {
      window.setContentSize(width, 760);
      for (const [visual, call] of [["agent", "agentWorkspaceVisual"], ["agent-routines", "agentRoutineVisual"],
        ["agent-profile", "agentProfileVisual"], ["agent-roster", "agentRosterVisual"], ["agent-memory", "agentMemoryVisual"],
        ["agent-computer", "agentComputerVisual"]]) {
        const name = `${visual}-${theme}-${width}`;
        report.visuals[name] = await js(window, `window.${call}(${JSON.stringify(theme)})`);
        await shot(window, name);
      }
    }
  }
  report.cliPaneAppearance = [];
  const cliPaneAppearance = async (style, theme, chrome, width, beforeCapture) => {
    const state = await js(window, `window.cliPaneAppearanceFixture(${JSON.stringify(style)}, ${JSON.stringify(theme)}, ${JSON.stringify(chrome)})`);
    assert.equal(state.panes, 4);
    assert.equal(state.mounts, 4);
    assert.equal(state.disposals, 0);
    assert.equal(state.readingSize, "11.5");
    report.cliPaneAppearance.push(state);
    if (beforeCapture) await beforeCapture();
    await captureWhenPainted(window, `cli-panes-${style}-${theme}-${chrome}-${width}`, `${style}/${theme}: CLI appearance did not paint`);
    return state;
  };
  for (const theme of ["dark", "light"]) {
    for (const chrome of CHROMES) {
      for (const width of [1100, 640]) {
        window.setContentSize(width, 760);
        const first = theme === "dark" && chrome === "refined" && width === 1100;
        const terminal = await cliPaneAppearance("terminal", theme, chrome, width, first ? async () => {
          report["header-terminal"] = await exercisePaneHeaderHover(window, output, "terminal");
        } : undefined);
        if (!first) continue;
        // Clean is retired: one legacy stored preference must leave the same native panes.
        const legacy = await cliPaneAppearance("clean", theme, chrome, width);
        assert.ok(Math.abs(legacy.cellHeight - terminal.cellHeight) < 1, `Legacy Clean preference changed the shared native text metrics: ${theme}/${chrome}/${width}, terminal=${terminal.cellHeight}, clean=${legacy.cellHeight}`);
      }
    }
  }
  report.translucency = [];
  const alpha = color => color === "transparent" ? 0
    : color.startsWith("rgba") ? Number(color.match(/, ([\d.]+)\)$/)[1])
    : color.includes(" / ") ? Number(color.split(" / ")[1].replace(")", "")) : 1;
  window.setContentSize(1100, 760);
  for (const chrome of CHROMES) {
    for (const theme of themes) {
      for (const [terminal, sidebar] of [[0, 0], [0, 100], [50, 0], [100, 100]]) {
        const state = await js(window, `window.translucencyFixture(${JSON.stringify(theme)}, ${terminal}, ${sidebar}, ${JSON.stringify(chrome)})`);
        assert.equal(state.pane.opacity, "1");
        assert.equal(state.sidebar.opacity, "1");
        assert.equal(state.view.opacity, "1");
        assert.equal(alpha(state.pane.background), terminal === 0 ? 1 : terminal === 50 ? 0.61 : 0.22);
        assert.equal(alpha(state.sidebar.background), sidebar === 0 ? 1 : 0.15, `${theme}/${chrome}/${terminal}/${sidebar}: ${JSON.stringify(state)}`);
        assert.equal(alpha(state.projectHeader.background), sidebar === 0 ? 1 : 0, `${theme}: project header adds an opaque strip`);
        assert.equal(alpha(state.xterm.background), terminal === 0 ? 1 : 0);
        assert.equal(alpha(state.viewport.background), terminal === 0 ? 1 : 0);
        if (terminal > 0) {
          for (const key of ["body", "view", "canvas", "workspace"]) assert.equal(alpha(state[key].background), 0, `${theme}: opaque ${key}`);
        }
        // Dense tiles (owner, 2026-10-01): the head bands carry the focus
        // hierarchy, solid or at any glass strength.
        assert.notEqual(state.head.background, state.idleHead.background,
          `${theme}/${chrome}/${terminal}: the focused head band lost its step`);
        report.translucency.push({ theme, chrome, ...state });
        if (chrome === "refined" && THEMES.includes(theme) && terminal !== 50 && terminal === sidebar) {
          await shot(window, `translucency-${theme}-${terminal}`);
        }
      }
    }
  }
  await js(window, 'window.translucencyFixture("dark", 40, 60)');
  await js(window, 'document.getElementById("fixture-terminal").focus()');
  await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { type: "keyDown", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
  await window.webContents.debugger.sendCommand("Input.dispatchKeyEvent", { type: "keyUp", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
  assert.deepEqual(await js(window, '[document.getElementById("fixture-terminal").value, document.getElementById("fixture-sidebar").value]'), ["41", "60"]);
  await media(window, [{ name: "prefers-reduced-transparency", value: "reduce" }]);
  const reduced = await js(window, 'window.translucencyFixture("dark", 100, 100)');
  assert.equal(reduced.active, "0");
  assert.equal(alpha(reduced.pane.background), 1);
  assert.equal(alpha(reduced.sidebar.background), 1);
  assert.equal(alpha(reduced.projectHeader.background), 1);
  assert.equal(alpha(reduced.xterm.background), 1);
  report.reducedTransparency = reduced;
  await media(window, []);
  await js(window, 'window.translucencyFixture("dark", 0, 0)');
  report.materialVisibility = await exerciseMaterialVisibility(window);
  await fs.writeFile(path.join(output, "report.json"), JSON.stringify(report, null, 2));
  return { ...report, translucency: report.translucency.length, reducedTransparency: "passed", terminalThemes: Object.keys(report.terminalThemes), visuals: Object.keys(report.visuals) };
}

app.whenReady().then(async () => {
  const { MAC_WINDOW_MATERIAL } = await import(pathToFileURL(path.join(__dirname, "../dist-main/main/window-material.js")).href);
  const window = new BrowserWindow({ show: false, width: 1100, height: 760,
    ...(process.platform === "darwin" ? MAC_WINDOW_MATERIAL : {}),
    webPreferences: { preload: path.join(__dirname, "../dist-main/main/preload.cjs"),
      contextIsolation: true, sandbox: true, nodeIntegration: false, backgroundThrottling: false,
      webviewTag: lane === "browser-point" || lane === "browser-chrome",
      // As main.ts sets it on the app window.
      disableHtmlFullscreenWindowResize: lane === "browser-chrome" } });
  window.webContents.on("render-process-gone", (_, details) => {
    console.error(`Fixture renderer exited: ${details.reason}`);
    app.exit(1);
  });
  await window.loadURL(url);
  window.webContents.debugger.attach("1.3");
  // Exercise real focus/blur handlers without showing a test window or stealing
  // focus from the owner's app. Hidden Chromium documents suppress them otherwise.
  await window.webContents.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: true });
  await media(window, [motion("reduce"), { name: "prefers-reduced-transparency", value: "no-preference" }]);
  const deadline = Date.now() + 30000;
  while (!await js(window, "typeof window.runRendererFixtures === 'function'")) {
    if (Date.now() > deadline) throw new Error("Fixture module failed to initialize");
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  const report = await RUN[lane](window);
  console.log(JSON.stringify({ ...report, artifacts: output }));
  window.destroy();
  app.exit(0);
}).catch(fail);

async function exercisePaneHeaderHover(window, output, style) {
  const read = () => window.webContents.executeJavaScript(`(() => {
    const head = document.querySelector('.terminal-pane.is-focused .terminal-pane-head');
    const rect = head.getBoundingClientRect();
    const css = getComputedStyle(head);
    const frame = getComputedStyle(head.closest('.terminal-pane'));
    return { background: css.backgroundColor, frame: frame.backgroundColor, shadow: css.boxShadow,
      hovered: head.matches(':hover'), keyboardFocus: head.matches(':has(:focus-visible)'),
      x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2) };
  })()`);
  const move = async (x, y) => {
    await window.webContents.debugger.sendCommand('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
    await window.webContents.executeJavaScript('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  };
  await move(1, 1);
  const selected = await read();
  // Dense tiles (owner, 2026-10-01): the head is a band over the screen, and
  // the focused pane's band steps up to Porcelain Night's #1e1d1b over the
  // screen-coloured tile.
  assert.equal(selected.background, 'rgb(30, 29, 27)', `${style}: selection lost its stepped-up head band`);
  assert.equal(selected.frame, 'rgb(10, 10, 9)', `${style}: the tile is no longer the screen's colour`);
  assert.equal(selected.hovered, false);
  assert.equal(selected.shadow, 'none', `${style}: selection alone acquired the purple hover wash`);
  await fs.writeFile(path.join(output, `header-${style}-selected.png`), (await window.webContents.capturePage()).toPNG());

  // Real keyboard modality must keep the control's focus ring without tinting its header.
  await window.webContents.debugger.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
  await window.webContents.debugger.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
  await window.webContents.executeJavaScript("document.querySelector('.terminal-pane.is-focused .terminal-tab').focus()");
  const keyboard = await read();
  assert.equal(keyboard.keyboardFocus, true);
  assert.equal(keyboard.background, selected.background);
  assert.equal(keyboard.shadow, 'none', `${style}: keyboard focus acquired the purple hover wash`);

  await move(selected.x, selected.y);
  const hovered = await read();
  assert.equal(hovered.hovered, true);
  assert.equal(hovered.background, selected.background);
  assert.ok(hovered.shadow.includes('rgba(139, 126, 189, 0.16)'), `${style}: hover lost its purple wash`);
  await fs.writeFile(path.join(output, `header-${style}-hovered.png`), (await window.webContents.capturePage()).toPNG());
  await move(1, 1);
  const left = await read();
  assert.equal(left.keyboardFocus, true);
  assert.equal(left.hovered, false);
  assert.equal(left.background, selected.background);
  assert.equal(left.shadow, 'none', `${style}: purple persisted after the pointer left`);
  await window.webContents.executeJavaScript('document.activeElement.blur()');
  return { selected, keyboard, hovered, left };
}

async function exerciseMaterialVisibility(window) {
  const { installWindowVisibility, syncWindowVisibility } = await import(pathToFileURL(path.join(__dirname, "../dist-main/main/window-visibility.js")).href);
  const { applyWindowMaterial } = await import(pathToFileURL(path.join(__dirname, "../dist-main/main/window-material.js")).href);
  await window.webContents.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: false });
  await window.webContents.debugger.sendCommand("Emulation.setEmulatedMedia", { features: [
    { name: "prefers-reduced-motion", value: "no-preference" },
    { name: "prefers-reduced-transparency", value: "no-preference" },
  ] });
  // The native show/minimize cycle below really orders the window in. Fully
  // transparent and click-through, it never appears on or intercepts the
  // owner's screen; capturePage reads the web contents, not the window alpha.
  window.setOpacity(0);
  window.setIgnoreMouseEvents(true);
  installWindowVisibility(window);
  const report = [];
  for (const chrome of ["refined", "cli"]) {
    for (const percent of [100, 0, 50]) {
      window.hide();
      syncWindowVisibility(window);
      assert.equal(window.webContents.backgroundThrottling, true, "Hidden windows must retain throttling");
      applyWindowMaterial(window, "#111214", { version: 1, terminal: percent, sidebar: percent });
      window.showInactive();
      syncWindowVisibility(window);
      assert.equal(window.webContents.backgroundThrottling, false, "Shown unfocused window stayed throttled");
      assert.equal(window.getOpacity(), 0, "The native visibility cycle must stay invisible to the owner");
      const state = await window.webContents.executeJavaScript(`window.translucencyFixture("dark", ${percent}, ${percent}, ${JSON.stringify(chrome)}, false)`);
      assert.equal(await window.webContents.executeJavaScript("document.visibilityState"), "visible");
      assert.equal(state.pane.opacity, "1");
      const frames = await window.webContents.executeJavaScript("window.readableSurfaceFrames()");
      // Paint an opaque Settings-like card, remove it, and require the WebContents
      // alpha surface to clear it. The final live app check also inspects the
      // native compositor behind this capture (not included by capturePage).
      await window.webContents.executeJavaScript("window.materialOverlayProbe(true)");
      await window.webContents.capturePage();
      await window.webContents.executeJavaScript("window.materialOverlayProbe(false)");
      const cleared = await window.webContents.capturePage();
      const size = cleared.getSize();
      const pixel = cleared.crop({ x: Math.floor(size.width * 0.75),
        y: Math.floor(size.height * 0.6), width: 1, height: 1 }).toBitmap();
      assert.ok(!(pixel[0] > 230 && pixel[2] > 230 && pixel[1] < 30),
        "Dismissed opaque card remains in the WebContents capture");
      await fs.writeFile(path.join(output, `material-cleared-${chrome}-${percent}.png`), cleared.toPNG());
      const scrolling = percent > 0 ? await window.webContents.executeJavaScript("window.sidebarGlassScrollFixture()") : [];
      if (percent === 0) {
        assert.equal(await window.webContents.executeJavaScript(`[...document.querySelectorAll(".sidebar .group > .session-items")]
          .filter(rows => rows.style.getPropertyValue("--sticky-header-clip") !== "").length`), 0,
          "A solid sidebar kept measuring sticky header clips");
      }
      report.push({ chrome, percent, frames, scrolling });
    }
  }
  // Actual native minimize/restore events, without focus emulation. Poll native
  // state because the macOS minimize animation completes asynchronously.
  window.minimize();
  const waitUntil = async predicate => {
    const deadline = Date.now() + 3000;
    while (!predicate()) {
      assert.ok(Date.now() < deadline, "Native visibility transition did not finish");
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  await waitUntil(() => window.isMinimized() && window.webContents.backgroundThrottling);
  window.restore();
  await waitUntil(() => !window.isMinimized() && !window.webContents.backgroundThrottling);
  await window.webContents.executeJavaScript("window.readableSurfaceFrames()");
  window.hide();
  syncWindowVisibility(window);
  assert.equal(window.webContents.backgroundThrottling, true);
  return { cycles: report, minimizeRestore: "passed", focusEmulation: false, motion: "normal" };
}

async function exerciseTerminalPort(window) {
  const load = name => import(pathToFileURL(path.join(__dirname, "../dist-main/main/", name)).href);
  const { TerminalDataTransport, TerminalDataDecoder, TerminalDataDirection, TerminalDataKind, encodeTerminalDataFrame }
    = await load("terminal-data-transport.js");
  const { TerminalOutputWindow, TERMINAL_OUTPUT_WINDOW_BYTES } = await load("terminal-output-window.js");
  const noisy = "00000000-0000-4000-8000-000000000001";
  const sibling = "00000000-0000-4000-8000-000000000002";
  const output = new PassThrough();
  const input = new PassThrough();
  const transport = new TerminalDataTransport(output, input);
  const failures = [];
  transport.onFailure(error => failures.push(error.message));
  const { port1, port2 } = new MessageChannelMain();
  const detach = transport.attachPort(port1);
  const token = randomUUID();
  const ready = new Promise(resolve => {
    const ack = (event, value) => {
      if (event.sender === window.webContents && value?.token === token) {
        ipcMain.off("iface:terminalDataPortAck", ack); resolve();
      }
    };
    ipcMain.on("iface:terminalDataPortAck", ack);
  });
  window.webContents.postMessage("iface:terminalDataPort", { version: 1, token }, [port2]);
  await ready;
  await window.webContents.executeJavaScript("window.beginTerminalPortFixture()");
  const credit = new TerminalOutputWindow();
  const decoder = new TerminalDataDecoder(TerminalDataDirection.ElectronToHost);
  const consumed = new Map();
  let next = 0;
  let eof = false;
  let peak = 0;
  let checksum = 0;
  let siblingLatency = null;
  const started = Date.now();
  const send = (terminalId, sequence, payload, kind = TerminalDataKind.Data) => {
    const frame = { terminalId, generation: 1n, sequence: BigInt(sequence),
      direction: TerminalDataDirection.HostToElectron, kind, payload };
    credit.sent(frame, payload.byteLength);
    peak = Math.max(peak, credit.outstandingBytes);
    output.write(encodeTerminalDataFrame(frame));
  };
  const pump = () => {
    while (next < 512 && credit.outstandingBytes + 4140 <= TERMINAL_OUTPUT_WINDOW_BYTES) {
      const payload = Buffer.alloc(4096, 120);
      const marker = Buffer.from("\r\nframe:" + next + "\r\n");
      marker.copy(payload, payload.length - marker.length);
      for (const byte of payload) checksum = (checksum + byte) >>> 0;
      send(noisy, next++, payload);
    }
    if (next === 512 && !eof && credit.outstandingBytes + 44 <= TERMINAL_OUTPUT_WINDOW_BYTES) {
      eof = true; send(noisy, 512, Buffer.alloc(0), TerminalDataKind.Eof);
    }
  };
  input.on("data", bytes => {
    for (const frame of decoder.push(bytes)) {
      if (frame.kind === TerminalDataKind.OutputAck) {
        const previous = consumed.get(frame.terminalId) ?? -1n;
        for (let sequence = previous + 1n; sequence <= frame.sequence; sequence++) {
          credit.complete({ ...frame, sequence });
        }
        consumed.set(frame.terminalId, frame.sequence);
        queueMicrotask(pump);
      } else if (frame.terminalId === sibling) {
        assert.equal(Buffer.from(frame.payload).toString(), "ping");
        siblingLatency = Date.now() - started;
      }
    }
  });
  send(sibling, 0, Buffer.from("interactive sibling\r\n"));
  pump();
  const result = await window.webContents.executeJavaScript("window.finishTerminalPortFixture()");
  // Finish means xterm consumed every byte; allow the last port ack to arrive.
  while (credit.outstandingBytes !== 0 || siblingLatency === null) {
    assert.ok(Date.now() - started < 10000, "terminal fixture did not drain");
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  assert.equal(result.checksum, checksum);
  assert.equal(result.frames, 512);
  assert.ok(peak <= TERMINAL_OUTPUT_WINDOW_BYTES + 128, "downstream memory did not plateau");
  assert.ok(siblingLatency < 1000, "noisy output blocked sibling input");
  assert.deepEqual(failures, []);
  detach(); port1.close(); transport.close();
  return { ...result, peakOutstandingBytes: peak, siblingInputLatencyMs: siblingLatency, durationMs: Date.now() - started };
}
