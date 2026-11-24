import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { runInNewContext } from "node:vm";
import {
  PACKAGED_FAKE_PROVIDERS, PACKAGED_LAUNCH_PROFILE_IDS, PACKAGED_RENDERER_PROBE,
  PACKAGED_RENDERER_VIEWPORT, PACKAGED_UPDATER_PROBE, RENDERER_PINS, packagedSmokeLaunch,
  parseSmokeCliArgs, publicChannelCompiledIn, validatePackagedRendererProbe,
} from "./smoke-packaged.mjs";
import { packagedSandboxEnvironment } from "./packaged-sandbox.mjs";

// Every assertion below that names a renderer literal reads it back out of the
// pin table, so the probe, this test and smoke-pins.test.mjs cannot drift apart.
const pin = literal => {
  assert.ok(RENDERER_PINS.some(candidate => candidate.literal === literal),
    `"${literal}" is asserted against the renderer but is not in RENDERER_PINS`);
  return literal;
};

const good = {
  viewport: PACKAGED_RENDERER_VIEWPORT,
  title: "Interface", readyState: "complete", hasRoot: true, hasIface: true, sessionCount: 0,
  adversarialReviewGuarded: true,
  defaultDirectory: "/private/tmp/profile/home/Desktop/Projects", repositoryName: "Projects",
  updatePhase: "disabled",
  providerLogo: "claude",
  modelSelectorLabel: "Model: Claude · Opus 5",
  commandButtonLabel: "Claude commands for claude-opus-5",
  commandButtonWidth: 24,
  surfaceSwitchLabels: ["Agent", "Code", "Threads"],
  usageEngineIds: ["claude", "codex", "grok", "acp:gemini"],
  geminiUsageWindows: [
    { label: "5-hour", percent: "60% used" },
    { label: "Weekly", percent: "25% used" },
  ],
  launchProfileIds: ["shell", "claude", "codex", "grok", "gemini", "omp", "opencode", "muse", "kimi"],
  passOffProfileIds: ["claude", "codex", "grok", "gemini", "omp", "opencode", "kimi"],
  launchProfileFieldsSafe: true,
  launcherProfiles: ["claude", "codex", "grok", "gemini", "omp", "opencode", "muse", "kimi", "shell"],
  launcherCounts: [1, 2, 3, 4, 5, 6, 7, 8],
  launcherLogos: ["claude", "codex", "grok", "antigravity", "omp", "opencode", "muse", "kimi"],
  launcherSectionLabels: ["Agent", "How many", "Isolation", "Working directory", "Will launch"],
  interfaceMarkCount: 1,
  launcherSingleActionLabel: "Launch 1 Claude Code terminal",
  launcherReviewLineup: "Claude Code builds, then Codex reviews",
  launcherReviewActionLabel: "Start review",
  launcherReviewNeedsObjective: true,
  launcherReviewCanStart: true,
  launcherReviewDismissed: true,
  launcherSixCountChecked: true,
  launcherActionLabel: "Launch 6 Claude Code terminals",
  launcherCwd: "/private/tmp/profile/home/Desktop/Projects",
  claudeBatchCount: 6,
  preInputTerminalPaneCount: 6,
  preInputReadyCount: 6,
  plainClickFocused: true,
  plainClickDidNotEmitInput: true,
  claudeBatchEchoCount: 6,
  hotkeySwitchedTerminal: true,
  overlayHotkeyIgnored: true,
  initialRenderedTerminalEchoCount: 6,
  cardsCoveredPaneCount: 6,
  cardsReturnEchoCount: 6,
  workspaceCreated: true,
  workspaceRoundTripCount: 3,
  workspaceReturnEchoCount: 6,
  truncatedAgentTargetCount: 1,
  truncatedAgentInitialOutputBytes: 1_114_112,
  truncatedAgentPreservedRoundTripCount: 3,
  workspaceAttemptDiagnostics: Array.from({ length: 3 }, () => ({
    retainedViewCount: 6, retainedFrameRendered: true, liveEchoCount: 6,
  })),
  terminalDataFailures: [],
  terminalPaneCount: 6,
  terminalPaneWidths: [320, 320, 320, 320, 320, 320],
  terminalPaneHeights: [240, 240, 240, 240, 240, 240],
  terminalGridColumns: 3,
  terminalGridRows: 2,
  terminalRemovalRows: [
    { count: 3, left: 12, right: 988, widths: [317, 317, 318] },
    { count: 2, left: 12, right: 988, widths: [482, 482] },
  ],
  terminalPaneFocusedStates: [true, false, false, false, false, false],
  terminalPaneHeaderHeights: [32, 32, 32, 32, 32, 32],
};

test("launcher readiness accepts all nine profiles and diagnoses an incomplete roster", async () => {
  const start = PACKAGED_RENDERER_PROBE.indexOf("const terminalLauncher = await waitFor(");
  const end = PACKAGED_RENDERER_PROBE.indexOf("const launcherProfiles =", start);
  assert.ok(start >= 0 && end > start);
  for (const count of [7, 8, 9]) {
    const launcher = { querySelectorAll: () => Array.from({ length: count }) };
    const result = runInNewContext(`(async () => {
      ${PACKAGED_RENDERER_PROBE.slice(start, end)}
      return terminalLauncher;
    })()`, {
      document: { querySelector: () => launcher },
      waitFor: async read => read(),
    });
    if (count === 9) assert.equal(await result, launcher);
    else await assert.rejects(result, /complete launch-profile catalog/);
  }
});

test("composer smoke creates the first agent, then opens its folder-scoped Agent composer", async () => {
  const start = PACKAGED_RENDERER_PROBE.indexOf("const agentSurface =");
  const end = PACKAGED_RENDERER_PROBE.indexOf("const expectedRepositoryName =", start);
  assert.ok(start > 0 && end > start);
  const clicks = [];
  const typed = [];
  class HTMLInputElement {
    set value(text) { typed.push(text); }
    dispatchEvent(event) { typed.push(event.type); }
  }
  const name = new HTMLInputElement();
  const dialog = { querySelector(selector) {
    if (selector === `input.${pin("agent-dialog-input")}`) return name;
    assert.equal(selector, `.${pin("agent-dialog-submit")}`);
    return typed.length === 2 ? { disabled: false, click() { clicks.push("Create"); } } : null;
  } };
  await runInNewContext(`(async () => { ${PACKAGED_RENDERER_PROBE.slice(start, end)} })()`, {
    HTMLInputElement, Event: class { constructor(type) { this.type = type; } },
    document: { querySelector(selector) {
      if (selector.includes(`aria-label="${pin("Agent")}"`)) return { click() { clicks.push("Agent"); } };
      if (selector === `.${pin("agent-dialog")}`) return clicks.includes("New agent") ? dialog : null;
      if (selector.includes(`aria-label="${pin("New agent")}"`)) {
        assert.ok(selector.endsWith(":not(:disabled)"));
        return { click() { clicks.push("New agent"); } };
      }
      assert.ok(selector.includes(`aria-label="${pin("New task")}"`));
      assert.deepEqual(clicks, ["Agent", "New agent", "Create"]);
      return { click() { clicks.push("New task"); } };
    } },
    waitFor: async read => read() ?? read(),
  });
  assert.deepEqual(typed, ["Packaged smoke agent", "input"]);
  assert.deepEqual(clicks, ["Agent", "New agent", "Create", "New task"]);
});

test("the probe's terminal waits poll the host and stop at a bridge failure", async () => {
  const slice = (from, to) => {
    const start = PACKAGED_RENDERER_PROBE.indexOf(from);
    const end = PACKAGED_RENDERER_PROBE.indexOf(to, start);
    assert.ok(start >= 0 && end > start, `${from} moved`);
    return PACKAGED_RENDERER_PROBE.slice(start, end);
  };
  const helper = slice("const waitUntil = async", "const sessions = await api.listSessions();");
  const launch = slice("const launchedClaude = await waitUntil(",
    "reportProgress('six-terminals-running');");
  const handoff = slice("await waitUntil(async () => {\n    const snapshot = await api.listTerminals();\n    const claudeGone",
    "reportProgress('six-terminals-exited');");
  const claude = title => ({ id: title, title, cwd: "/p", state: "running",
    launch: { version: 1, profileId: "claude" } });
  const six = ["Claude 6", "Claude 2", "Claude 4", "Claude 1", "Claude 5", "Claude 3"].map(claude);
  const context = (terminalFailures, snapshots, clock = Date) => {
    let calls = 0;
    return {
      terminalFailures, Date: clock, setTimeout: resolve => { resolve(); },
      api: { listTerminals: async () => snapshots[Math.min(calls++, snapshots.length - 1)] },
    };
  };
  const launched = await runInNewContext(`(async () => { ${helper} ${launch} return launchedClaude; })()`,
    context([], [{ terminals: six.slice(0, 5) }, { terminals: six }]));
  assert.deepEqual(launched.map(terminal => terminal.title),
    ["Claude 1", "Claude 2", "Claude 3", "Claude 4", "Claude 5", "Claude 6"]);
  await assert.rejects(
    runInNewContext(`(async () => { ${helper} ${launch} })()`,
      context(["port lost"], [{ terminals: six }])),
    /^Error: Terminal data bridge failed: port lost$/u);
  let now = 0;
  await assert.rejects(
    runInNewContext(`(async () => { ${helper} ${launch} })()`,
      context([], [{ terminals: [] }], { now: () => (now += 5_000) })),
    /^Error: Six packaged Claude terminals did not reach running state\.$/u);
  // The shell hand-off after the Claude CLIs exit never consulted the bridge.
  const shells = six.map(terminal => ({ ...terminal, id: `shell-${terminal.id}`,
    launch: { version: 1, profileId: "shell" } }));
  await runInNewContext(`(async () => { ${helper} ${handoff} })()`, {
    ...context(["port lost"], [{ terminals: [...six, ...shells] }, { terminals: shells }]),
    launchedClaude: six,
  });
});

test("workspace smoke chooses New workspace from the Workspaces plus menu", async () => {
  const start = PACKAGED_RENDERER_PROBE.indexOf("newWorkspaceButton?.click();");
  const end = PACKAGED_RENDERER_PROBE.indexOf("reportProgress('workspace-created-visible'", start);
  assert.ok(start >= 0 && end > start);
  let activeId = "original";
  const clicks = [];
  const createdId = await runInNewContext(`(async () => {
    ${PACKAGED_RENDERER_PROBE.slice(start, end)}
    return createdWorkspaceId;
  })()`, {
    attempt: 0, originalWorkspaceId: "original",
    newWorkspaceButton: { click() { clicks.push("plus"); } },
    document: { querySelector(selector) {
      assert.equal(selector, `[role="menu"][aria-label="${pin("New workspace or project")}"]`);
      assert.deepEqual(clicks, ["plus"]);
      return { querySelectorAll(itemSelector) {
        assert.equal(itemSelector, '[role="menuitem"]');
        return [
          { textContent: "New project…", disabled: false, click() { assert.fail("wrong action"); } },
          { textContent: `\n  ${pin("New workspace")}\n`, disabled: false,
            click() { clicks.push("workspace"); activeId = "created"; } },
        ];
      } };
    } },
    window: { localStorage: { getItem() { return activeId; } } },
    waitFor: async read => read(), reportProgress() {},
  });
  assert.equal(createdId, "created");
  assert.deepEqual(clicks, ["plus", "workspace"]);
  // Execute the real report too: menu diagnostics must not reference a stale
  // variable that only throws after the packaged workspace round trip.
  const diagnosticStart = PACKAGED_RENDERER_PROBE.indexOf("const attemptDiagnostic = {");
  const diagnosticEnd = PACKAGED_RENDERER_PROBE.indexOf("\n      };", diagnosticStart) + 9;
  assert.ok(diagnosticStart > end && diagnosticEnd > diagnosticStart);
  const diagnostic = runInNewContext(`${PACKAGED_RENDERER_PROBE.slice(diagnosticStart, diagnosticEnd)}\nattemptDiagnostic;`, {
    attempt: 0, newWorkspaceButton: {}, newWorkspaceMenuItem: {}, createdWorkspaceId: createdId,
    createdWorkspaceVisible: true, returnWorkspaceButton: {}, returned: true,
    retainedFrame: "fake-claude-frame:24x80:1",
  });
  assert.equal(diagnostic.createdWorkspaceId, "created");
  assert.equal(diagnostic.newWorkspaceMenuItemFound, true);
  assert.equal(diagnostic.returned, true);
});

test("host, launcher, pass-off and logos must retain OMP and OpenCode", () => {
  for (const profile of ["omp", "opencode"]) {
    for (const field of ["launchProfileIds", "launcherProfiles", "passOffProfileIds", "launcherLogos"]) {
      assert.throws(() => validatePackagedRendererProbe({
        ...good, [field]: good[field].filter(id => id !== profile),
      }), /launch-profile catalog|agent roster/);
    }
  }
});

test("host, launcher and logos retain Muse Code, and Pass off never offers it", () => {
  for (const field of ["launchProfileIds", "launcherProfiles", "launcherLogos"]) {
    assert.throws(() => validatePackagedRendererProbe({
      ...good, [field]: good[field].filter(id => id !== "muse"),
    }), /launch-profile catalog|agent roster/);
  }
  assert.throws(() => validatePackagedRendererProbe({
    ...good, passOffProfileIds: [...good.passOffProfileIds, "muse"],
  }), /Pass off/);
});

// 0.3.173's first packaging stopped here: Kimi Code had no fake `kimi`, so
// the host projected it unavailable and the launcher never showed it.
test("the sandbox fakes a CLI for every agent profile the launcher must show", () => {
  for (const id of PACKAGED_LAUNCH_PROFILE_IDS.filter(id => id !== "shell")) {
    const command = id === "gemini" ? "agy" : id;
    assert.ok(Object.hasOwn(PACKAGED_FAKE_PROVIDERS, command), `no fake ${command} for ${id}`);
  }
  for (const fake of Object.values(PACKAGED_FAKE_PROVIDERS)) {
    assert.ok(["claude", "grok", "qwen", "antigravity", "qwenMlxServer"].includes(fake), fake);
  }
});

test("host, launcher, logos and Pass off retain Kimi Code", () => {
  for (const field of ["launchProfileIds", "launcherProfiles", "launcherLogos"]) {
    assert.throws(() => validatePackagedRendererProbe({
      ...good, [field]: good[field].filter(id => id !== "kimi"),
    }), /launch-profile catalog|agent roster/);
  }
  assert.throws(() => validatePackagedRendererProbe({
    ...good, passOffProfileIds: good.passOffProfileIds.filter(id => id !== "kimi"),
  }), /Pass off/);
});

test("the packaged probe releases output credits after consuming data and EOF", () => {
  const start = PACKAGED_RENDERER_PROBE.indexOf("const terminalOutput = new Map();");
  const end = PACKAGED_RENDERER_PROBE.indexOf("const sessions = await api.listSessions();");
  assert.ok(start >= 0 && end > start);
  let listener;
  const output = runInNewContext(`${PACKAGED_RENDERER_PROBE.slice(start, end)}\nterminalOutput;`, {
    TextDecoder,
    api: {
      onTerminalData: callback => { listener = callback; return () => {}; },
      onTerminalDataFailure: () => () => {},
    },
  });
  // Exceed the transport window just as the packaged redraw fixture does.
  const data = new TextEncoder().encode("x".repeat(4096));
  let acknowledgements = 0;
  for (let frame = 0; frame < 512; frame++) {
    listener({ terminalId: "probe", kind: "data", data }, () => {
      assert.equal(output.get("probe").length, (frame + 1) * data.length);
      acknowledgements++;
    });
    assert.equal(acknowledgements, frame + 1);
  }
  listener({ terminalId: "probe", kind: "eof" }, () => { acknowledgements++; });
  assert.equal(acknowledgements, 513);
  assert.throws(() => listener({ terminalId: "probe", kind: "data", data: {} }, () => {
    acknowledgements++;
  }));
  assert.equal(acknowledgements, 514, "a decoder failure must not strand a credit");
});

test("terminal removal probe waits for full-width geometry after the exit ghost disappears", () => {
  const marker = "const terminalRemovalRows = await waitFor(";
  const start = PACKAGED_RENDERER_PROBE.indexOf(marker) + marker.length;
  const end = PACKAGED_RENDERER_PROBE.indexOf("\n  });", start);
  assert.ok(start >= marker.length && end > start);
  const pollSource = PACKAGED_RENDERER_PROBE.slice(start, end) + "\n  }";
  const rect = (left, top, width) => ({ left, top, width, right: left + width });
  const upper = [rect(0, 0, 300), rect(312, 0, 300), rect(624, 0, 300)];
  let frame = [...upper, rect(0, 200, 410), rect(470, 200, 410)];
  const poll = runInNewContext(`(${pollSource})`, {
    document: { querySelectorAll: () => frame.map(bounds => ({
      getBoundingClientRect: () => bounds,
    })) },
  });
  assert.equal(poll(), null, "five panes can still be resizing after the ghost exits");
  frame = [...upper, rect(0, 200, 456), rect(468, 200, 456)];
  const settled = poll();
  assert.equal(settled.length, 2);
  assert.equal(settled[1].right, settled[0].right);
  frame = [...upper, rect(0, 200, 410), rect(468, 200, 456)];
  assert.equal(poll(), null, "aligned outer edges must not hide unequal pane widths");
});

test("packaged probe covers document, preload, Rust host IPC, and updater IPC", () => {
  assert.match(PACKAGED_RENDERER_PROBE, /document\.querySelector\('#root'\)/);
  assert.match(PACKAGED_RENDERER_PROBE, /api\.listSessions\(\)/);
  assert.match(PACKAGED_RENDERER_PROBE, /api\.getDefaultDirectory\(\)/);
  assert.match(PACKAGED_RENDERER_PROBE, /api\.getUpdateState\(\)/);
  assert.match(PACKAGED_RENDERER_PROBE, /terminalRemovalRows/);
  assert.match(PACKAGED_RENDERER_PROBE, /api\.listTerminalLaunchProfiles\(\)/);
  assert.match(PACKAGED_RENDERER_PROBE, /api\.startAdversarialReview\(/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    pin("cross-model Claude/Codex pair").toLowerCase()));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("data-terminal-launch-profile")));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("terminal-launcher-count")));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("terminal-launcher-review-link")));
  for (const selector of ["adversarial-review-dialog", "adversarial-review-objective",
    "adversarial-review-submit", "adversarial-review-lineup"]) {
    assert.ok(PACKAGED_RENDERER_PROBE.includes(`.${pin(selector)}`));
  }
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`'${pin("Start review")}'`));
  assert.match(PACKAGED_RENDERER_PROBE, /sixChoice\?\.click\(\)/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`Launch 1 ${pin("Claude Code")} terminal`));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`Launch 6 ${pin("Claude Code")} terminals`));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /data-terminal-launch-preset/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /data-terminal-launch-mode/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /data-terminal-review-role/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /terminal-launcher-summary/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /terminal-launcher-objective/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /replace\(\/s\+\//);
  assert.match(PACKAGED_RENDERER_PROBE, /fake-claude-echo:probe-/);
  assert.match(PACKAGED_RENDERER_PROBE, /preInputReadyCount/);
  assert.match(PACKAGED_RENDERER_PROBE, /plain-click-sentinel/);
  assert.match(PACKAGED_RENDERER_PROBE, /plainClickDidNotEmitInput/);
  assert.match(PACKAGED_RENDERER_PROBE, /new PointerEvent\('pointerdown'/);
  assert.match(PACKAGED_RENDERER_PROBE, /new MouseEvent\('mouseup'/);
  assert.match(PACKAGED_RENDERER_PROBE, /button: 0, detail: 1/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    "output.match(/fake-claude-echo:[^\\r\\n]*plain-click-sentinel/u)",
  ));
  assert.match(PACKAGED_RENDERER_PROBE, /getBoundingClientRect/);
  assert.match(PACKAGED_RENDERER_PROBE, /terminalPaneFocusedStates/);
  assert.match(PACKAGED_RENDERER_PROBE,
    /const paneFocusedStates = terminalPanes\.map\(pane =>\s*pane\.classList\.contains\('is-focused'\)\)/u);
  assert.ok(PACKAGED_RENDERER_PROBE.indexOf("const paneFocusedStates")
    < PACKAGED_RENDERER_PROBE.indexOf("closeForReflow.click()"));
  // Look values belong to the renderer DOM gate (review F2, 2026-10-15): the
  // packaged probe no longer reads gutters, corners, borders or shadows.
  for (const look of [/terminalGridColumnGap/, /terminalPaneBoxShadows/, /style\.boxShadow/,
    /borderTopLeftRadius/, /surfaceSwitchCenterDelta/, /launcherAgentColumns/,
    /modelSelectorWidth/]) {
    assert.doesNotMatch(PACKAGED_RENDERER_PROBE, look);
  }
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /terminalPaneRingKinds/);
  assert.match(PACKAGED_RENDERER_PROBE, /terminalPaneHeaderHeights/);
  // The pane header's two × are told apart by name: the right-hand one closes
  // the active terminal, the per-tab one closes the tab it sits in.
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`.${pin("terminal-pane-action")}[aria-label="${
    pin("Close active terminal (ends it)")}"]`));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /\[title="Close tab"\]/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("data-provider-logo")));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("composer-command-button")));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    `.${pin("surface-switch")} > .${pin("surface-switch-tab")}`));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`.${pin("surface-switch")} > .${
    pin("surface-switch-tab")}[aria-label="${pin("Code")}"]`));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /Open integrated terminals/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /workspace-toolbar-destinations/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    `[${pin("data-interface-mark")}="${pin("fold")}"]`));
  // Settings is opened by the toolbar gear, which the deleted sidebar footer
  // used to hold, and closed by the ✕ that replaces it.
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`button[aria-label="${pin("Settings")}"]`));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    `button[aria-label="${pin("Close settings")}"]`));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /aria-label="Open settings"/);
  // Flip is a row of the window's ··· menu: the probe opens it each way.
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`button[aria-label="${pin("More workspace options")}"]`));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("Flip terminals")));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /Show terminal cards/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("terminal-pane-card")));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /Show terminal overview/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(`.${pin("sidebar-primary-nav")}`));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("New workspace")));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /New terminal workspace/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    pin("interface.integrated-terminals.workspace.v1")));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("terminal-launcher")));
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("data-terminal-ready")));
  assert.match(PACKAGED_RENDERER_PROBE, /workspaceAttemptDiagnostics/);
  assert.match(PACKAGED_RENDERER_PROBE, /retainedViewCount/);
  assert.match(PACKAGED_RENDERER_PROBE, /retainedFrameRendered/);
  assert.match(PACKAGED_RENDERER_PROBE, /liveEchoCount/);
  assert.match(PACKAGED_RENDERER_PROBE, /fake-claude-frame:/);
  assert.match(PACKAGED_RENDERER_PROBE, /truncatedAgentPreservedRoundTripCount/);
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /redrawShrinkMarker|redrawFinalMarker/);
  assert.match(PACKAGED_RENDERER_PROBE, /__interfacePackagedSmokeProgress/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(pin("data-terminal-workspace-id")));
  assert.doesNotMatch(PACKAGED_RENDERER_PROBE, /terminal-workspace-tab/);
  assert.ok(PACKAGED_RENDERER_PROBE.includes(
    `${pin("data-engine")}="${pin("acp:gemini")}"`));
  assert.match(PACKAGED_RENDERER_PROBE, /BracketRight/);
  assert.doesNotThrow(() => new Function(`return ${PACKAGED_RENDERER_PROBE}`));
  assert.deepEqual(validatePackagedRendererProbe(good), good);
});

test("packaged desktop smoke rejects restored-window and pre-compact UI assumptions", () => {
  for (const viewport of [undefined, { width: 1440, height: 900 }]) {
    assert.throws(() => validatePackagedRendererProbe({ ...good, viewport }),
      /pinned desktop viewport/);
  }
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherCounts: [1, 2, 3, 4, 5, 6] }), /1 through 8/);
});

test("the packaged app launches hidden-safe on the private smoke root and fake providers", () => {
  const profile = "/private/tmp/interface-rust-packaged-smoke-x";
  const launch = packagedSmokeLaunch("/Volumes/Release/Interface.app", profile, 9333);
  assert.equal(launch.executable, "/Volumes/Release/Interface.app/Contents/MacOS/Interface");
  assert.deepEqual(launch.args, [
    `--user-data-dir=${profile}`,
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=9333",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
  ]);
  const fakeHome = `${profile}/home`;
  // Only the sandbox session environment plus the smoke's own variables: no
  // inherited variable, no test data root, and HOME stays the process home.
  assert.deepEqual(launch.options.env, {
    ...packagedSandboxEnvironment(profile),
    INTERFACE_HOST_SMOKE_DATA_ROOT: `${profile}/data`,
    INTERFACE_HOST_SMOKE_HOME: fakeHome,
    INTERFACE_PACKAGED_SMOKE_REDRAW_CLAIM: `${profile}/fake-claude-redraw-claim`,
    CLAUDE_CODE_EXECUTABLE: `${profile}/fake-claude`,
    CODEX_EXECUTABLE: `${profile}/fake-claude`,
    ANTIGRAVITY_EXECUTABLE: `${fakeHome}/.local/bin/agy`,
    GROK_EXECUTABLE: `${profile}/fake-grok`,
    QWEN_EXECUTABLE: `${profile}/fake-qwen`,
    QWEN_MLX_SERVER_EXECUTABLE: `${profile}/fake-qwen-mlx-server`,
  });
  assert.equal(launch.options.env.HOME, `${profile}/process-home`);
  assert.equal("INTERFACE_HOST_TEST_DATA_ROOT" in launch.options.env, false);
  assert.ok(Object.keys(launch.options.env).every(key => !/GEMINI/u.test(key)));
  assert.deepEqual(launch.options.stdio, ["ignore", "pipe", "pipe"]);
  assert.equal(launch.options.shell, false);
  assert.equal(launch.options.cwd, fakeHome);
});

test("smoke implementation refuses release hosts without the private-root marker", async () => {
  const source = await import("node:fs/promises").then(({ readFile }) =>
    readFile(new URL("./smoke-packaged.mjs", import.meta.url), "utf8"));
  assert.match(source, /const SMOKE_ROOT_MARKER = "INTERFACE_HOST_SMOKE_DATA_ROOT";/);
  assert.match(source, /x\.ai\/models\/list/);
  assert.match(source, /_x\.ai\/billing/);
  assert.match(source, /claim=\$\{INTERFACE_PACKAGED_SMOKE_REDRAW_CLAIM:-/);
  assert.match(source, /\[ -t 0 \]/);
  assert.match(source, /read -r -t 1/);
  assert.match(source, /frame_rows=\$\{dimensions%% \*\}/);
  assert.match(source, /\/bin\/dd if=\/dev\/zero bs=65536 count=17/);
  assert.doesNotMatch(source, /fakeGemini/);
  assert.match(source, /does not support a private release-smoke data root/);
  assert.match(source, /result\.defaultDirectory !== expectedDefaultDirectory/);
});

test("a packaged build with a public update channel must come up with updates on", async () => {
  const required = { requireUpdater: true };
  // The probe's first read may come before the updater is configured; only
  // the settled phase counts, and "disabled" there means a refused signature.
  for (const settledUpdatePhase of ["idle", "error", "up-to-date"]) {
    const value = { ...good, settledUpdatePhase };
    assert.equal(validatePackagedRendererProbe(value, [], required), value);
  }
  for (const settledUpdatePhase of ["disabled", null, undefined]) {
    assert.throws(() => validatePackagedRendererProbe({ ...good, settledUpdatePhase }, [], required),
      /packaged updater is off/);
  }
  // A build without a channel keeps the bridge-only check.
  assert.deepEqual(validatePackagedRendererProbe(good), good);

  const channel = await readFile(
    new URL("../../crates/interface-updater/release-channel.json", import.meta.url), "utf8");
  assert.equal(publicChannelCompiledIn(channel), true);
  for (const [feeds, keys] of [[[], ["key"]], [["feed"], []], [undefined, ["key"]]]) {
    assert.equal(publicChannelCompiledIn(JSON.stringify({ schema: 1, feeds, keys })), false);
  }
  assert.throws(() => publicChannelCompiledIn("{"), /not valid JSON/);

  // The settle probe polls the host's updater state past "disabled".
  const phases = ["disabled", "disabled", "idle"];
  let reads = 0;
  const settled = await runInNewContext(PACKAGED_UPDATER_PROBE, {
    Date, setTimeout: resolve => { resolve(); },
    window: { iface: { getUpdateState: async () => ({ phase: phases[Math.min(reads++, 2)] }) } },
  });
  assert.equal(settled, "idle");
  assert.equal(reads, 3);
  let now = 0;
  assert.equal(await runInNewContext(PACKAGED_UPDATER_PROBE, {
    Date: { now: () => (now += 10_000) }, setTimeout: resolve => { resolve(); },
    window: { iface: { getUpdateState: async () => ({ phase: "disabled" }) } },
  }), "disabled");
});

test("packaged probe rejects missing UI, preload, IPC, and renderer errors", () => {
  assert.throws(() => validatePackagedRendererProbe({ ...good, hasRoot: false }), /root/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, hasIface: false }), /preload/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, sessionCount: -1 }), /IPC/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, defaultDirectory: "" }),
    /default directory/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, updatePhase: null }), /updater-state/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, repositoryName: "Elsewhere" }),
    /composer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, providerLogo: null }),
    /provider logo/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, modelSelectorLabel: null }),
    /concrete model/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    modelSelectorLabel: "Provider: Claude" }), /concrete model/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    modelSelectorLabel: "Model: Choose model" }), /concrete model/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, commandButtonLabel: "Claude" }),
    /provider commands/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, commandButtonWidth: 0 }),
    /provider commands/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    surfaceSwitchLabels: ["Threads", "Code"] }), /Threads, Code, and Agent/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    surfaceSwitchLabels: ["Threads", "Code", "Agent"] }), /Threads, Code, and Agent/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    surfaceSwitchLabels: ["Agent", "Code", "Chat"] }), /Threads, Code, and Agent/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    usageEngineIds: ["acp:qwen", "acp:gemini"] }), /omit Qwen/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launchProfileIds: [] }),
    /launch-profile catalog/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    passOffProfileIds: ["claude", "codex", "gemini"] }), /Pass off/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherLogos: [] }),
    /owner's agent roster/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherSectionLabels: [] }),
    /session-composer structure/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherSectionLabels: ["Agent", "How many", "Working directory", "Will launch"] }),
    /session-composer structure/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, interfaceMarkCount: 0 }),
    /Fold mark/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherSingleActionLabel: "" }),
    /launch button/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherReviewLineup: null }),
    /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherReviewLineup: "Codex builds, then Codex reviews" }), /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherReviewNeedsObjective: false }), /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherReviewActionLabel: "Start adversarial review" }), /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherReviewCanStart: false }), /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    launcherReviewDismissed: false }), /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    adversarialReviewGuarded: false }), /builder and reviewer/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherSixCountChecked: false }),
    /six-terminal selection/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherActionLabel: "Launch" }),
    /six-terminal selection/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherCounts: [1, 2] }),
    /1 through 8/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, launcherCwd: "/elsewhere" }),
    /working directory/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, claudeBatchEchoCount: 5 }),
    /PTY data round-trip/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, plainClickFocused: false }),
    /plain click/);
  assert.throws(() => validatePackagedRendererProbe({
    ...good, plainClickDidNotEmitInput: false,
  }), /plain click/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, overlayHotkeyIgnored: false }),
    /terminal hotkey/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, cardsCoveredPaneCount: 0 }),
    /card-flip and workspace transitions.*"cardsCoveredPaneCount":0/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, cardsReturnEchoCount: 0 }),
    /card-flip and workspace transitions.*"cardsReturnEchoCount":0/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, workspaceReturnEchoCount: 0 }),
    /card-flip and workspace transitions.*"workspaceReturnEchoCount":0/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, workspaceRoundTripCount: 2 }),
    /card-flip and workspace transitions/);
  assert.throws(() => validatePackagedRendererProbe({
    ...good, truncatedAgentTargetCount: 0,
  }), /truncated-agent preservation/);
  assert.throws(() => validatePackagedRendererProbe({
    ...good, truncatedAgentInitialOutputBytes: 1024 * 1024,
  }), /truncated-agent preservation/);
  assert.throws(() => validatePackagedRendererProbe({
    ...good, truncatedAgentPreservedRoundTripCount: 2,
  }), /truncated-agent preservation/);
  for (const failure of [
    { retainedViewCount: 5 }, { retainedFrameRendered: false }, { liveEchoCount: 5 },
  ]) {
    assert.throws(() => validatePackagedRendererProbe({
      ...good, workspaceAttemptDiagnostics: good.workspaceAttemptDiagnostics.map(
        (entry, index) => index === 1 ? { ...entry, ...failure } : entry),
    }), /truncated-agent preservation/);
  }
  assert.throws(() => validatePackagedRendererProbe({ ...good, terminalPaneCount: 0 }),
    /card-flip and workspace transitions/);
  assert.throws(() => validatePackagedRendererProbe({ ...good, terminalGridColumns: 2 }),
    /3 by 2 grid/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    terminalPaneWidths: [320, 320, 480, 320, 320, 480] }), /equal dimensions/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    terminalRemovalRows: [
      good.terminalRemovalRows[0],
      { ...good.terminalRemovalRows[1], right: 824, widths: [400, 400] },
    ],
  }), /full-width 3 \+ 2 grid/);
  assert.throws(() => validatePackagedRendererProbe({ ...good,
    terminalPaneFocusedStates: [false, false, false, false, false, false] }),
    /one focused tile/);
  for (const height of [30, 34]) {
    assert.throws(() => validatePackagedRendererProbe({ ...good,
      terminalPaneHeaderHeights: [32, 32, 32, height, 32, 32] }), /equal heads/);
  }
  // A new head height is a look change, not a packaging failure.
  assert.doesNotThrow(() => validatePackagedRendererProbe({ ...good,
    terminalPaneHeaderHeights: [40, 40, 40, 40, 40, 40] }));
  assert.throws(() => validatePackagedRendererProbe(good, ["boom"]), /fatal error/);
});

// The contract only holds while the probe is assembled from RENDERER_PINS: a
// literal typed straight into the probe is one nobody can check in a second.
// A bare quoted string is only one way it gets there — most of the probe's
// literals sit inside a selector, so those spellings are checked as well.
const spellingsOf = literal => [
  `'${literal}'`, `"${literal}"`,   // a bare value the probe compares against
  `#${literal}`,                    // an id selector
  `[${literal}]`, `[${literal}=`,   // an attribute selector named by the pin
  `="${literal}"`, `='${literal}'`, // an attribute selector matched by value
  // A class selector, but only for kebab-case names: `.claude` in a one-word
  // pin is `value.claudeBatchCount`, not a selector anyone typed.
  ...(literal.includes("-") ? [`.${literal}`] : []),
];

test("no pinned literal is spelled out in the probe or its validations", async () => {
  const source = await readFile(new URL("./smoke-packaged.mjs", import.meta.url), "utf8");
  // Reads from the table are what the contract asks for, so they come out
  // first: without that, `PIN.rootId` reads as the selector `.root`.
  const region = source.slice(
    source.indexOf("export const PACKAGED_RENDERER_PROBE"),
    source.indexOf("async function inspectRenderer"),
  ).replaceAll(/\$\{[^{}]*\}/gu, "").replaceAll(/\bPIN\.\w+/gu, "");
  assert.ok(region.length > 0, "the probe and its validator moved");
  for (const { literal } of RENDERER_PINS) {
    for (const spelling of spellingsOf(literal)) {
      assert.ok(!region.includes(spelling),
        `${JSON.stringify(spelling)} is typed into the probe; read ${
          JSON.stringify(literal)} from the pin table instead`);
    }
  }
  // The selector spellings are the ones a bare-quote check used to miss.
  assert.ok(spellingsOf("terminal-pane").includes(".terminal-pane"));
  assert.ok(spellingsOf("Settings").includes('="Settings"'));
  assert.ok(spellingsOf("data-engine").includes("[data-engine="));
});

test("smoke CLI accepts exactly one app bundle path", () => {
  assert.equal(parseSmokeCliArgs(["/tmp/Interface.app"]).appPath, "/tmp/Interface.app");
  assert.throws(() => parseSmokeCliArgs([]), /Usage/);
  assert.throws(() => parseSmokeCliArgs(["one", "two"]), /Usage/);
});
