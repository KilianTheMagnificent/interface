import { spawn, spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { packagedSandboxEnvironment, packagedSandboxPolicy } from "./packaged-sandbox.mjs";

const DEFAULT_TIMEOUT_MS = 60_000;
const PROFILE_PREFIX = "interface-rust-packaged-smoke-";
const SMOKE_ROOT_MARKER = "INTERFACE_HOST_SMOKE_DATA_ROOT";
// The Rust host compiles this file in; the packaged binaries come from the
// same checkout the smoke runs from.
const RELEASE_CHANNEL_FILE = new URL("../../crates/interface-updater/release-channel.json", import.meta.url);
const UPDATER_SETTLE_MS = 30_000;

/**
 * Whether the packaged Rust host follows a public update channel: the rule of
 * `ReleaseChannel::parse`, at least one feed and at least one key.
 */
export function publicChannelCompiledIn(text) {
  let file;
  try {
    file = JSON.parse(text);
  } catch {
    fail("crates/interface-updater/release-channel.json is not valid JSON.");
  }
  return Array.isArray(file?.feeds) && file.feeds.length > 0
    && Array.isArray(file?.keys) && file.keys.length > 0;
}

// The Rust updater reads "disabled" until Electron's configure call has read
// and checked the running app's signature (two codesign runs over the whole
// bundle), so the probe's first read may come too early. This waits for it to
// settle; a build whose signature the updater refuses stays "disabled".
export const PACKAGED_UPDATER_PROBE = `
(async () => {
  const started = Date.now();
  let phase = null;
  while (Date.now() - started < ${UPDATER_SETTLE_MS}) {
    phase = (await window.iface.getUpdateState())?.phase ?? null;
    if (phase !== null && phase !== 'disabled') return phase;
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  return phase;
})()
`;

// The renderer pin contract.
//
// A packaged smoke costs about thirteen minutes, and on 2026-09-04 one of them
// spent all of it to discover that the probe still selected
// `.terminal-pane-action[title="Close tab"]` after the pane header was renamed.
// Every selector, attribute name, attribute value and visible string the probe
// or validatePackagedRendererProbe() asserts against the renderer is declared
// once below, next to the file that carries it, and the probe and the
// validator are built from these constants — a literal cannot reach either
// without a pin. `smoke-pins.test.mjs` reads every pinned file and fails in
// about a second when a literal has moved or been renamed.
//
// `file` is relative to `electron/src/renderer/`. `repo: true` marks a literal
// the renderer does not own — host-projected labels and wire ids that come from
// Rust, the bundle's own `index.html`, and the three xterm.js classes the
// terminal DOM inherits from the library — and its path is relative to the
// repository root.
//
// `context` is the enclosing source text the test searches for instead of the
// bare literal, and it is what makes a pin able to fail at all. `terminal-pane`
// on its own survives a rename because `terminal-pane-head` still contains it;
// `Settings` survives because App.tsx says the word forty other times. The test
// enforces this rather than trusting the table: a literal that occurs more than
// once in its file MUST carry a context, and every context must contain its own
// literal, so the two cannot drift apart.
//
// Two kinds of literal are deliberately absent:
//   * Composed strings (the launcher's action label, the usage percentage) are
//     pinned by the source fragment that builds them, because the whole visible
//     sentence exists only at runtime; the helpers below reassemble them.
//   * Negative assertions ("acp:qwen" must be gone, "Close tab" must not come
//     back) and serde-generated wire names ("unavailableReason", the terminal
//     "running" state) are not pinned: a pin proves a literal is present in a
//     named file, and those have no verbatim source to point at.
const XTERM_LIB = "electron/node_modules/@xterm/xterm/lib/xterm.js";
const PINS = {
  launcherCountLimit: { literal: "8", file: "components/TerminalLauncher.tsx",
    context: "Array.from({ length: 8 }, (_, index) => index + 1)",
  },
  // The packaged document itself, which lives outside src/renderer/.
  documentTitle: { literal: "Interface", file: "electron/index.html", repo: true,
    context: "<title>Interface</title>",
  },
  rootId: { literal: "root", file: "electron/index.html", repo: true,
    context: '<div id="root">',
  },

  // xterm.js owns the terminal's inner DOM; the renderer only styles it.
  xterm: { literal: "xterm", file: "terminal.css",
    context: ".terminal-xterm .xterm {",
  },
  xtermScreen: { literal: "xterm-screen", file: XTERM_LIB, repo: true },
  xtermHelperTextarea: { literal: "xterm-helper-textarea", file: XTERM_LIB, repo: true },
  xtermAccessibilityTree: { literal: "xterm-accessibility-tree", file: XTERM_LIB, repo: true },

  terminalView: { literal: "terminal-view", file: "components/TerminalView.tsx",
    context: "className={`terminal-view${",
  },
  terminalReadyAttr: { literal: "data-terminal-ready", file: "components/TerminalView.tsx" },
  terminalIdAttr: { literal: "data-terminal-id", file: "components/TerminalView.tsx" },

  terminalPane: { literal: "terminal-pane", file: "components/TerminalPane.tsx",
    context: "className={`terminal-pane${focused",
  },
  terminalPaneFocused: { literal: "is-focused", file: "components/TerminalPane.tsx" },
  terminalPaneIdAttr: { literal: "data-terminal-pane-id", file: "components/TerminalPane.tsx" },
  terminalPaneHead: { literal: "terminal-pane-head", file: "components/TerminalPane.tsx" },
  terminalPaneAction: { literal: "terminal-pane-action", file: "components/TerminalPane.tsx",
    context: 'className="terminal-pane-action"',
  },
  // One noun per object: the header's right-hand × closes the active terminal
  // and says so, which is what tells it apart from the per-tab × beside it.
  closeActiveTerminal: {
    literal: "Close active terminal (ends it)", file: "components/TerminalPane.tsx",
    context: 'aria-label="Close active terminal (ends it)"',
  },
  morePaneActions: { literal: "More pane actions", file: "components/TerminalPane.tsx",
    context: 'aria-label="More pane actions"',
  },
  paneActionsMenu: { literal: "Pane actions", file: "components/TerminalPane.tsx",
    context: 'aria-label="Pane actions"',
  },
  passOffAction: { literal: "Pass off to another agent", file: "components/TerminalPane.tsx",
    context: 'aria-label="Pass off to another agent"',
  },
  passOffMenu: { literal: "Pass off to", file: "components/TerminalPane.tsx",
    context: 'aria-label="Pass off to"',
  },
  terminalPaneCard: { literal: "terminal-pane-card", file: "components/TerminalPane.tsx",
    context: 'className="terminal-pane-card"',
  },
  passOffProfileAttr: {
    literal: "data-terminal-launch-profile", file: "components/TerminalPaneNewMenu.tsx",
  },

  terminalCenterSplit: {
    literal: "terminal-center-split", file: "components/TerminalWorkspace.tsx",
  },
  // Layout, Usage and Attention open from the window's ··· menu (owner, 2026-09-26).
  moreWorkspaceOptions: { literal: "More workspace options", file: "components/WorkspaceMoreMenu.tsx" },
  workspaceMenu: { literal: "Workspace menu", file: "components/WorkspaceMoreMenu.tsx" },
  flipTerminals: { literal: "Flip terminals", file: "components/TerminalWorkspace.tsx" },

  terminalLauncher: { literal: "terminal-launcher", file: "components/TerminalLauncher.tsx",
    context: 'className="terminal-launcher"',
  },
  launchProfileAttr: {
    literal: "data-terminal-launch-profile", file: "components/TerminalLauncher.tsx",
  },
  launcherCount: { literal: "terminal-launcher-count", file: "components/TerminalLauncher.tsx",
    context: "className={`terminal-launcher-count${",
  },
  launcherProfile: {
    literal: "terminal-launcher-profile", file: "components/TerminalLauncher.tsx",
    context: "className={`terminal-launcher-profile${",
  },
  launcherSection: {
    literal: "terminal-launcher-section", file: "components/TerminalLauncher.tsx",
    context: 'className="terminal-launcher-section"',
  },
  launcherCwdPath: {
    literal: "terminal-launcher-cwd-path", file: "components/TerminalLauncher.tsx",
  },
  launcherActions: {
    literal: "terminal-launcher-actions", file: "components/TerminalLauncher.tsx",
  },
  launcherPrimaryButton: { literal: "btn-primary", file: "components/TerminalLauncher.tsx" },
  launcherReviewLink: {
    literal: "terminal-launcher-review-link", file: "components/TerminalLauncher.tsx",
  },
  launcherAgentHeading: { literal: "Agent", file: "components/TerminalLauncher.tsx",
    context: "}>Agent</h3>",
  },
  launcherCountHeading: { literal: "How many", file: "components/TerminalLauncher.tsx",
    context: "}>How many</h3>",
  },
  launcherIsolationHeading: { literal: "Isolation", file: "components/TerminalLauncher.tsx",
    context: "}>Isolation</h3>",
  },
  launcherPreviewHeading: { literal: "Will launch", file: "components/TerminalLauncher.tsx",
    context: "}>Will launch</h3>",
  },
  launcherCwdHeading: { literal: "Working directory", file: "components/TerminalLauncher.tsx",
    context: "}>Working directory</h3>",
  },
  launcherActionTemplate: {
    literal: "Launch ${count.toString()} ${subject} terminal",
    file: "components/TerminalLauncher.tsx",
  },

  providerLogoAttr: { literal: "data-provider-logo", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="',
  },
  claudeLogo: { literal: "claude", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="claude"',
  },
  codexLogo: { literal: "codex", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="codex"',
  },
  grokLogo: { literal: "grok", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="grok"',
  },
  antigravityLogo: { literal: "antigravity", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="antigravity"',
  },
  ompLogo: { literal: "omp", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="omp"',
  },
  opencodeLogo: { literal: "opencode", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="opencode"',
  },
  museLogo: { literal: "muse", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="muse"',
  },
  kimiLogo: { literal: "kimi", file: "components/ProviderLogo.tsx",
    context: 'data-provider-logo="kimi"',
  },

  reviewDialog: {
    literal: "adversarial-review-dialog", file: "components/AdversarialReviewDialog.tsx",
    context: 'className="adversarial-review-dialog is-floating"',
  },
  reviewObjective: {
    literal: "adversarial-review-objective", file: "components/AdversarialReviewDialog.tsx",
    context: 'className="adversarial-review-objective"',
  },
  reviewSubmit: {
    literal: "adversarial-review-submit", file: "components/AdversarialReviewDialog.tsx",
    context: 'className="adversarial-review-button adversarial-review-submit"',
  },
  reviewLineup: {
    literal: "adversarial-review-lineup", file: "components/AdversarialReviewDialog.tsx",
    context: 'className="adversarial-review-lineup"',
  },
  reviewActions: {
    literal: "adversarial-review-actions", file: "components/AdversarialReviewDialog.tsx",
    context: 'className="adversarial-review-actions"',
  },
  reviewLineupText: {
    literal: "Claude Code builds, then Codex reviews",
    file: "components/AdversarialReviewDialog.tsx",
  },
  reviewStart: { literal: "Start review", file: "components/AdversarialReviewDialog.tsx" },
  reviewCancel: { literal: "Cancel", file: "components/AdversarialReviewDialog.tsx",
    context: "onClick={onCancel}>Cancel</button>",
  },

  interfaceMarkAttr: { literal: "data-interface-mark", file: "components/InterfaceMark.tsx",
    context: 'data-interface-mark="fold"',
  },
  interfaceMarkFold: { literal: "fold", file: "components/InterfaceMark.tsx",
    context: 'data-interface-mark="fold"',
  },

  surfaceSwitch: { literal: "surface-switch", file: "components/SurfaceSwitch.tsx",
    context: 'className="surface-switch" aria-label="Workspace"',
  },
  surfaceSwitchTab: { literal: "surface-switch-tab", file: "components/SurfaceSwitch.tsx",
    context: '"surface-switch-tab is-active" : "surface-switch-tab"',
  },
  surfaceChat: { literal: "Threads", file: "components/SurfaceSwitch.tsx",
    context: ': "Threads"',
  },
  surfaceCode: { literal: "Code", file: "components/SurfaceSwitch.tsx",
    context: '? "Code"',
  },
  surfaceAgent: { literal: "Agent", file: "components/SurfaceSwitch.tsx",
    context: '? "Agent"',
  },
  // Settings lives below Workspaces in the persistent sidebar footer.
  settingsButton: { literal: "Settings", file: "components/Sidebar.tsx",
    context: 'aria-label="Settings"',
  },
  closeSettingsButton: { literal: "Close settings", file: "App.tsx",
    context: 'aria-label="Close settings"',
  },
  settingsOverlay: { literal: "workspace-settings-overlay", file: "App.tsx",
    context: 'className="workspace-settings-overlay"',
  },
  newWorkspace: { literal: "New workspace", file: "App.tsx", context: 'code: "New workspace"' },
  workspaceCreateMenu: { literal: "New workspace or project", file: "components/Sidebar.tsx",
    context: 'aria-label={auxMenu.kind === "create" ? "New workspace or project"',
  },
  newWorkspaceMenuItem: { literal: "New workspace", file: "components/Sidebar.tsx",
    context: 'onClick={() => { setAuxMenu(null); onNew(); }}>\n                  New workspace',
  },
  newTask: { literal: "New task", file: "App.tsx", context: 'agent: "New task"' },
  newAgent: { literal: "New agent", file: "components/AgentMenu.tsx", context: 'aria-label="New agent"' },
  agentDialog: { literal: "agent-dialog", file: "components/AgentMenu.tsx",
    context: 'className="agent-dialog is-floating"',
  },
  agentDialogName: { literal: "agent-dialog-input", file: "components/AgentMenu.tsx",
    context: 'id={`${baseId}-name`} className="agent-dialog-input"',
  },
  agentDialogSubmit: { literal: "agent-dialog-submit", file: "components/AgentMenu.tsx",
    context: 'className="agent-dialog-button agent-dialog-submit"',
  },

  usageMenuRow: { literal: "Usage", file: "components/WorkspaceMoreMenu.tsx",
    context: '<span>Usage</span>',
  },
  usagePanel: { literal: "usage-menu-panel", file: "components/UsageMenu.tsx" },
  usageHead: { literal: "usage-head", file: "components/UsageMenu.tsx",
    context: 'className="usage-head"',
  },
  usageRefresh: { literal: "Refresh", file: "components/UsageMenu.tsx",
    context: ': "Refresh"}',
  },
  usageProvider: { literal: "usage-provider", file: "components/UsageMenu.tsx",
    context: '"usage-provider" : "usage-provider is-unavailable"',
  },
  usageEngineAttr: { literal: "data-engine", file: "components/UsageMenu.tsx" },
  usageWindow: { literal: "usage-window", file: "components/UsageMenu.tsx",
    context: 'className="usage-window" key=',
  },
  usageWindowLabel: { literal: "usage-window-label", file: "components/UsageMenu.tsx" },
  usagePct: { literal: "usage-pct", file: "components/UsageMenu.tsx",
    context: ': "usage-pct"}',
  },
  usagePercentTemplate: {
    literal: "${limit.percentText} used", file: "components/UsageMenu.tsx",
    context: ": `${limit.percentText} used`}",
  },
  geminiEngineId: { literal: "acp:gemini", file: "provider-labels.ts" },

  composerContextName: { literal: "composer-context-name", file: "components/Composer.tsx",
    context: 'className="composer-context-name"',
  },
  composerModelPicker: { literal: "composer-model-picker", file: "components/Composer.tsx",
    context: 'className="composer-app-picker composer-model-picker"',
  },
  composerModelPopover: { literal: "composer-model-popover", file: "components/Composer.tsx",
    context: 'popoverClassName="composer-model-popover"',
  },
  composerCommandButton: { literal: "composer-command-button", file: "components/Composer.tsx",
    context: 'className="composer-icon-btn composer-command-button"',
  },
  composerCommandsFor: { literal: "commands for", file: "components/Composer.tsx",
    context: "commands for ${selectedModel}",
  },
  composerModelLabel: { literal: "Model", file: "components/Composer.tsx",
    context: 'label="Model"',
  },
  composerModelPlaceholder: { literal: "Choose model", file: "components/Composer.tsx",
    context: 'placeholder="Choose model"',
  },
  appSelectTrigger: { literal: "app-select-trigger", file: "components/AppSelect.tsx",
    context: 'className="app-select-trigger"',
  },
  appSelectLeading: { literal: "app-select-leading", file: "components/AppSelect.tsx",
    context: 'className="app-select-leading"',
  },
  appSelectAddOption: { literal: "codicon-add", file: "components/AppSelect.tsx",
    context: "codicon codicon-sm codicon-add app-select-check",
  },

  sidebarPrimaryNav: { literal: "sidebar-primary-nav", file: "components/Sidebar.tsx",
    context: 'className="sidebar-primary-nav"',
  },
  workspaceIdAttr: { literal: "data-terminal-workspace-id", file: "components/Sidebar.tsx",
    context: 'data-terminal-workspace-id={record.id}',
  },
  activeWorkspaceAttr: { literal: "aria-current", file: "components/Sidebar.tsx",
    context: 'aria-current={current ? "page" : undefined}',
  },
  workspaceStorageKey: {
    literal: "interface.integrated-terminals.workspace.v1",
    file: "components/IntegratedTerminalCenter.tsx",
  },

  // Wire ids and labels the Rust host projects into the renderer.
  shellProfileId: {
    literal: "shell", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Shell => "shell"',
  },
  claudeProfileId: {
    literal: "claude", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Claude => "claude"',
  },
  codexProfileId: {
    literal: "codex", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Codex => "codex"',
  },
  grokProfileId: {
    literal: "grok", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Grok => "grok"',
  },
  geminiProfileId: {
    literal: "gemini", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Gemini => "gemini"',
  },
  ompProfileId: {
    literal: "omp", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Omp => "omp"',
  },
  opencodeProfileId: {
    literal: "opencode", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Opencode => "opencode"',
  },
  museProfileId: {
    literal: "muse", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Muse => "muse"',
  },
  kimiProfileId: {
    literal: "kimi", file: "crates/interface-contracts/src/terminal.rs", repo: true,
    context: 'Self::Kimi => "kimi"',
  },
  // The adversarial-review guard is tripped by naming the same engine twice.
  claudeEngineId: {
    literal: "claude", file: "crates/interface-terminal/src/launch.rs", repo: true,
    context: 'engine: Some("claude")',
  },
  claudeCodeLabel: {
    literal: "Claude Code", file: "crates/interface-terminal/src/launch.rs", repo: true,
    context: 'label: "Claude Code"',
  },
  geminiFiveHourWindow: {
    literal: "5-hour", file: "apps/interface-host/src/antigravity_usage.rs", repo: true,
    context: '("gemini-5h", "5-hour")',
  },
  geminiWeeklyWindow: {
    literal: "Weekly", file: "apps/interface-host/src/antigravity_usage.rs", repo: true,
    context: '("gemini-weekly", "Weekly")',
  },
  // The probe lowercases the rejected IPC error before matching, so the pin
  // keeps the Rust spelling and the probe lowercases it too.
  adversarialGuard: {
    literal: "cross-model Claude/Codex pair", file: "apps/interface-host/src/orchestration.rs",
    repo: true,
  },
};

export const RENDERER_PINS = Object.values(PINS);

const PIN = Object.fromEntries(
  Object.entries(PINS).map(([name, pin]) => [name, pin.literal]),
);

// One expected catalog drives both launcher readiness and result validation.
// smoke-pins.test.mjs checks it against the Rust owner's complete PROFILE_ORDER.
export const PACKAGED_LAUNCH_PROFILE_IDS = Object.freeze([
  PIN.shellProfileId, PIN.claudeProfileId, PIN.codexProfileId, PIN.grokProfileId,
  PIN.geminiProfileId, PIN.ompProfileId, PIN.opencodeProfileId, PIN.museProfileId,
  PIN.kimiProfileId,
]);
const PACKAGED_AGENT_PROFILE_IDS = PACKAGED_LAUNCH_PROFILE_IDS.filter(
  id => id !== PIN.shellProfileId,
);
// Each command the host's login-PATH lookup resolves, and the fake it runs in
// the sandbox. Every agent profile above needs one, or the host projects it as
// unavailable and the launcher never shows the complete catalog.
export const PACKAGED_FAKE_PROVIDERS = Object.freeze({
  claude: "claude", codex: "claude", grok: "grok", qwen: "qwen", agy: "antigravity",
  omp: "claude", opencode: "claude", muse: "claude", kimi: "claude",
  "qwen-mlx-server": "qwenMlxServer",
});
// Muse Code's TUI takes no initial prompt, so Pass off never offers it. Kimi
// Code's takes none either, but the host types the brief into it.
const PACKAGED_PASS_OFF_PROFILE_IDS = PACKAGED_AGENT_PROFILE_IDS.filter(
  id => id !== PIN.museProfileId,
);

// `launcherActionTemplate` and `usagePercentTemplate` pin the source fragments
// these two mirror; the visible sentences only exist once React has run.
function launchButtonLabel(count, subject) {
  return `Launch ${String(count)} ${subject} terminal${count === 1 ? "" : "s"}`;
}

function usageWindowPercent(percent) {
  return `${String(percent)}% used`;
}

function fail(message) { throw new Error(message); }

function boundedTimeout(value) {
  if (!Number.isFinite(value)) return DEFAULT_TIMEOUT_MS;
  return Math.max(5_000, Math.min(90_000, Math.floor(value)));
}

async function deadline(promise, timeoutMs, label) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} timed out.`)), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function reserveLoopbackPort() {
  const server = createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (address === null || typeof address === "string") fail("Could not reserve a smoke-test port.");
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return address.port;
}

async function waitForTarget(port, child, timeoutMs, fetchImpl) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (child.exitCode !== null) {
      fail(`Packaged Interface exited before its renderer was ready (code ${child.exitCode}).`);
    }
    try {
      const response = await fetchImpl(`http://127.0.0.1:${port}/json/list`);
      if (response.ok) {
        const targets = await response.json();
        if (Array.isArray(targets)) {
          const page = targets.find(target => target?.type === "page"
            && typeof target.webSocketDebuggerUrl === "string");
          if (page !== undefined) return page.webSocketDebuggerUrl;
        }
      }
    } catch { /* DevTools has not started listening yet. */ }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  fail("Packaged Interface did not expose a renderer before the smoke-test deadline.");
}

class CdpClient {
  constructor(url, WebSocketImpl) {
    this.socket = new WebSocketImpl(url);
    this.nextId = 1;
    this.pending = new Map();
    this.errors = [];
    this.ready = new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", () => reject(new Error(
        "Could not connect to the packaged renderer.")), { once: true });
    });
    this.socket.addEventListener("message", event => {
      let message;
      try { message = JSON.parse(String(event.data)); } catch { return; }
      if (message.method === "Runtime.exceptionThrown") {
        const description = message.params?.exceptionDetails?.exception?.description
          ?? message.params?.exceptionDetails?.text;
        this.errors.push(typeof description === "string" ? description : "Renderer exception");
      }
      if (message.method === "Log.entryAdded" && message.params?.entry?.level === "error") {
        this.errors.push(String(message.params.entry.text ?? "Renderer log error"));
      }
      if (typeof message.id !== "number") return;
      const waiting = this.pending.get(message.id);
      if (waiting === undefined) return;
      this.pending.delete(message.id);
      if (message.error !== undefined) waiting.reject(new Error(String(message.error.message)));
      else waiting.resolve(message.result);
    });
  }

  async call(method, params = {}) {
    await this.ready;
    const id = this.nextId++;
    const result = new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
    this.socket.send(JSON.stringify({ id, method, params }));
    return result;
  }

  close() { this.socket.close(); }
}

// Exercise desktop grid geometry independently of the display/window restored
// by macOS. The native renderer lane separately covers responsive widths.
export const PACKAGED_RENDERER_VIEWPORT = Object.freeze({ width: 1920, height: 1080 });

export const PACKAGED_RENDERER_PROBE = `
(async () => {
  const terminalMutationStats = { added: 0, removed: 0, readyChanges: 0 };
  const countViews = node => node instanceof Element
    ? Number(node.matches('.${PIN.terminalView}')) + node.querySelectorAll('.${PIN.terminalView}').length : 0;
  const terminalMutationObserver = new MutationObserver(records => {
    for (const record of records) {
      if (record.type === 'attributes') {
        terminalMutationStats.readyChanges += 1;
        continue;
      }
      for (const node of record.addedNodes) terminalMutationStats.added += countViews(node);
      for (const node of record.removedNodes) terminalMutationStats.removed += countViews(node);
    }
  });
  terminalMutationObserver.observe(document.documentElement, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ['${PIN.terminalReadyAttr}'],
  });
  const terminalDomState = () => ({
    terminalViewCount: document.querySelectorAll('.${PIN.terminalView}').length,
    xtermCount: document.querySelectorAll('.${PIN.terminalView} .${PIN.xterm}').length,
    readyViewCount: document.querySelectorAll('.${PIN.terminalView}[${PIN.terminalReadyAttr}]').length,
    terminalPaneCount: document.querySelectorAll('.${PIN.terminalPane}').length,
    terminalLauncherCount: document.querySelectorAll('.${PIN.terminalLauncher}').length,
    terminalSplitInert: document.querySelector('.${PIN.terminalCenterSplit}')?.inert ?? null,
    terminalViewAdds: terminalMutationStats.added,
    terminalViewRemovals: terminalMutationStats.removed,
    terminalReadyChanges: terminalMutationStats.readyChanges,
  });
  const reportProgress = (stage, detail = {}) => {
    globalThis.__interfacePackagedSmokeProgress = { stage, ...terminalDomState(), ...detail };
  };
  reportProgress('starting');
  const waitFor = async (read, timeoutMs = 20_000) => {
    const started = Date.now();
    while (Date.now() - started < timeoutMs) {
      const value = read();
      if (value !== null && value !== undefined) return value;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    return null;
  };
  const api = window.iface;
  const terminalOutput = new Map();
  const terminalFailures = [];
  const decoder = new TextDecoder();
  const removeTerminalData = api.onTerminalData((event, complete) => {
    try {
      if (event.kind !== 'data') return;
      terminalOutput.set(event.terminalId,
        (terminalOutput.get(event.terminalId) ?? '') + decoder.decode(event.data));
    } finally {
      complete();
    }
  });
  const removeTerminalFailure = api.onTerminalDataFailure(message => {
    terminalFailures.push(message);
  });
  // Polls an async check every 50 ms for 20 s and throws \`timeoutMessage\`;
  // unless \`bridge\` is false, a terminal data bridge failure ends it at once.
  const waitUntil = async (check, timeoutMessage, { bridge = true } = {}) => {
    const started = Date.now();
    while (Date.now() - started < 20_000) {
      if (bridge && terminalFailures.length > 0) {
        throw new Error('Terminal data bridge failed: ' + terminalFailures[0]);
      }
      const value = await check();
      if (value) return value;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error(timeoutMessage);
  };
  const sessions = await api.listSessions();
  const defaultDirectory = await api.getDefaultDirectory();
  const updateState = await api.getUpdateState();
  const launchProfiles = await api.listTerminalLaunchProfiles();
  reportProgress('initial-ipc-complete');
  const surfaceSwitchLabels = [...document.querySelectorAll(
    '.${PIN.surfaceSwitch} > .${PIN.surfaceSwitchTab}'
  )].map(button => button.getAttribute('aria-label') ?? button.textContent?.trim() ?? '');
  let adversarialReviewGuarded = false;
  if (typeof api.startAdversarialReview === 'function') {
    try {
      await api.startAdversarialReview({
        builderEngineId: '${PIN.claudeEngineId}',
        reviewerEngineId: '${PIN.claudeEngineId}',
        canonicalCwd: defaultDirectory,
        objective: 'Packaged IPC guard probe; no provider may start.',
        clientRequestId: crypto.randomUUID(),
      });
    } catch (error) {
      adversarialReviewGuarded = String(error).toLowerCase()
        .includes('${PIN.adversarialGuard.toLowerCase()}');
    }
  }
  // Code starts in the CLI workspace. Agent retains the folder-scoped native
  // composer; Chat deliberately omits repository controls.
  const agentSurface = document.querySelector('.${PIN.surfaceSwitch} > .${PIN.surfaceSwitchTab}[aria-label="${PIN.surfaceAgent}"]');
  agentSurface?.click();
  // A fresh profile has no agent, and Agent makes one before it takes a task.
  // New agent stays disabled until the Rust agent store has answered.
  const agentDialog = await waitFor(() => {
    const dialog = document.querySelector('.${PIN.agentDialog}');
    if (dialog !== null) return dialog;
    document.querySelector('button[aria-label="${PIN.newAgent}"]:not(:disabled)')?.click();
    return null;
  });
  if (agentDialog === null) throw new Error('Packaged Agent surface did not open New agent.');
  const agentName = agentDialog.querySelector('input.${PIN.agentDialogName}');
  if (agentName === null) throw new Error('Packaged New agent form has no name field.');
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
    ?.call(agentName, 'Packaged smoke agent');
  agentName.dispatchEvent(new Event('input', { bubbles: true }));
  const createAgent = await waitFor(() => {
    const button = agentDialog.querySelector('.${PIN.agentDialogSubmit}');
    return button !== null && !button.disabled ? button : null;
  });
  if (createAgent === null) throw new Error('Packaged New agent form did not accept a name.');
  createAgent.click();
  const newTaskButton = await waitFor(() => document.querySelector('button[aria-label="${PIN.newTask}"]'));
  if (newTaskButton === null) throw new Error('Packaged Agent surface did not expose New task.');
  newTaskButton.click();
  const expectedRepositoryName = defaultDirectory.split('/').filter(Boolean).at(-1) ?? null;
  const composerState = await waitFor(() => {
    const repositoryName = document.querySelector('.${PIN.composerContextName}')?.textContent?.trim()
      ?? null;
    const providerLogo = document.querySelector(
      '.${PIN.composerModelPicker} .${PIN.appSelectLeading} [${PIN.providerLogoAttr}]'
    )?.getAttribute('${PIN.providerLogoAttr}') ?? null;
    const modelSelector = document.querySelector('.${PIN.composerModelPicker} .${PIN.appSelectTrigger}');
    return repositoryName === expectedRepositoryName && providerLogo !== null
      && modelSelector !== null ? { repositoryName, providerLogo, modelSelector } : null;
  });
  const repositoryName = composerState?.repositoryName ?? null;
  const providerLogo = composerState?.providerLogo ?? null;
  const modelSelector = composerState?.modelSelector ?? null;
  const readyCommandButton = () => {
    const button = document.querySelector('.${PIN.composerCommandButton}');
    if (button === null || button.getBoundingClientRect().width < 24) return null;
    const label = button.getAttribute('aria-label') ?? "";
    return label.toLowerCase().includes('${PIN.composerCommandsFor.toLowerCase()}')
      ? button : null;
  };
  let commandButton = readyCommandButton();
  if (commandButton === null && modelSelector !== null) {
    modelSelector.click();
    const modelOption = await waitFor(() => [...document.querySelectorAll(
      '.${PIN.composerModelPopover} [role="option"]'
    )].find(row => row.hidden !== true
      && row.getAttribute('aria-disabled') !== 'true'
      && row.querySelector('.${PIN.appSelectAddOption}') === null
      && row.getClientRects().length > 0
      && getComputedStyle(row).visibility !== 'hidden') ?? null);
    modelOption?.click();
    commandButton = await waitFor(() => readyCommandButton());
  }
  const modelSelectorLabel = modelSelector?.getAttribute('aria-label') ?? null;
  const commandButtonLabel = commandButton?.getAttribute('aria-label') ?? null;
  const commandButtonWidth = commandButton?.getBoundingClientRect().width ?? 0;
  reportProgress('composer-ready');
  document.querySelector('button[aria-label="${PIN.moreWorkspaceOptions}"]')?.click();
  (await waitFor(() => [...document.querySelectorAll('[role="menu"][aria-label="${PIN.workspaceMenu}"] [role="menuitem"]')]
    .find(item => item.textContent.trim() === '${PIN.usageMenuRow}')))?.click();
  const usagePanel = await waitFor(() => {
    const panel = document.querySelector('.${PIN.usagePanel}');
    return panel !== null && panel.querySelector('.${PIN.usageHead} button')?.textContent?.trim()
      === '${PIN.usageRefresh}' ? panel : null;
  }, 25_000);
  const usageEngineIds = usagePanel === null ? [] : [...usagePanel.querySelectorAll(
    '.${PIN.usageProvider}[${PIN.usageEngineAttr}]'
  )].map(section => section.getAttribute('${PIN.usageEngineAttr}'));
  const geminiUsageWindows = usagePanel === null ? [] : [...usagePanel.querySelectorAll(
    '.${PIN.usageProvider}[${PIN.usageEngineAttr}="${PIN.geminiEngineId}"] .${PIN.usageWindow}'
  )].map(window => ({
    label: window.querySelector('.${PIN.usageWindowLabel}')?.textContent?.trim() ?? null,
    percent: window.querySelector('.${PIN.usagePct}')?.textContent?.trim() ?? null,
  }));
  // An outside press closes the usage panel, as it does for a person.
  document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
  document.querySelector('.${PIN.surfaceSwitch} > .${PIN.surfaceSwitchTab}[aria-label="${PIN.surfaceCode}"]')?.click();
  reportProgress('opening-terminal-launcher');
  const terminalLauncher = await waitFor(() => {
    const launcher = document.querySelector('.${PIN.terminalLauncher}');
    return launcher !== null
      && launcher.querySelectorAll('[${PIN.launchProfileAttr}]').length === ${PACKAGED_LAUNCH_PROFILE_IDS.length}
      ? launcher : null;
  });
  if (terminalLauncher === null) {
    throw new Error('Packaged terminal launcher did not expose the complete launch-profile catalog.');
  }
  const launcherProfiles = terminalLauncher === null ? [] : [...terminalLauncher.querySelectorAll(
    '[${PIN.launchProfileAttr}]'
  )].map(button => button.getAttribute('${PIN.launchProfileAttr}'));
  const launcherCounts = terminalLauncher === null ? [] : [...terminalLauncher.querySelectorAll(
    '.${PIN.launcherCount}'
  )].map(button => Number(button.textContent?.trim()));
  const launcherLogos = terminalLauncher === null ? [] : [...terminalLauncher.querySelectorAll(
    '.${PIN.launcherProfile} [${PIN.providerLogoAttr}]'
  )].map(mark => mark.getAttribute('${PIN.providerLogoAttr}'));
  const launcherSectionLabels = terminalLauncher === null ? [] : [...terminalLauncher.querySelectorAll(
    '.${PIN.launcherSection} > h3'
  )].map(heading => heading.textContent?.trim());
  const interfaceMarkCount = terminalLauncher?.querySelectorAll('[${PIN.interfaceMarkAttr}="${PIN.interfaceMarkFold}"]').length ?? 0;
  // The launch button retains the precise action while the lineup previews each seat.
  const launcherSingleActionLabel = await waitFor(() => {
    const action = terminalLauncher?.querySelector(
      '.${PIN.launcherActions} .${PIN.launcherPrimaryButton}'
    )?.textContent?.trim() ?? null;
    // The path is shown home-relative (~/…); its title carries the absolute one.
    const cwd = terminalLauncher?.querySelector('.${PIN.launcherCwdPath}')
      ?.getAttribute('title') ?? null;
    return action === '${launchButtonLabel(1, PIN.claudeCodeLabel)}' && cwd === defaultDirectory
      ? action : null;
  });
  const claudeChoice = terminalLauncher?.querySelector(
    '[${PIN.launchProfileAttr}="${PIN.claudeProfileId}"]'
  ) ?? null;
  const sixChoice = terminalLauncher === null ? null : [...terminalLauncher.querySelectorAll(
    '.${PIN.launcherCount}'
  )].find(button => button.textContent?.trim() === '6') ?? null;
  // Review left the launcher: one link opens a dialog that portals to the body,
  // so everything below is queried from the document, not from the launcher.
  terminalLauncher?.querySelector('.${PIN.launcherReviewLink}')?.click();
  const reviewState = await waitFor(() => {
    const dialog = document.querySelector('.${PIN.reviewDialog}');
    const objective = dialog?.querySelector('.${PIN.reviewObjective}') ?? null;
    const submit = dialog?.querySelector('.${PIN.reviewSubmit}') ?? null;
    return dialog !== null && objective !== null && submit !== null
      ? { dialog, objective, submit } : null;
  });
  const launcherReviewLineup = reviewState?.dialog
    .querySelector('.${PIN.reviewLineup}')?.textContent?.trim() ?? null;
  const launcherReviewActionLabel = reviewState?.submit.textContent?.trim() ?? null;
  const launcherReviewNeedsObjective = reviewState?.submit.disabled === true;
  const reviewObjective = reviewState?.objective ?? null;
  if (reviewObjective !== null) {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set
      ?.call(reviewObjective, 'Implement and verify the requested change.');
    reviewObjective.dispatchEvent(new Event('input', { bubbles: true }));
  }
  const launcherReviewCanStart = await waitFor(() => {
    const button = document.querySelector('.${PIN.reviewSubmit}');
    return button !== null && !button.disabled
      && button.textContent?.trim() === '${PIN.reviewStart}' ? true : null;
  }) === true;
  [...document.querySelectorAll('.${PIN.reviewActions} button')]
    .find(button => button.textContent?.trim() === '${PIN.reviewCancel}')?.click();
  const launcherReviewDismissed = await waitFor(() =>
    document.querySelector('.${PIN.reviewDialog}') === null ? true : null) === true;
  claudeChoice?.click();
  sixChoice?.click();
  const launchSix = await waitFor(() => {
    const button = terminalLauncher?.querySelector('.${PIN.launcherActions} .${PIN.launcherPrimaryButton}');
    return button !== null && !button.disabled
      && button.textContent?.trim() === '${launchButtonLabel(6, PIN.claudeCodeLabel)}' ? button : null;
  });
  const launcherSixCountChecked = sixChoice?.getAttribute('aria-checked') === 'true';
  const launcherActionLabel = launchSix?.textContent?.trim() ?? null;
  reportProgress('launching-six-terminals');
  launchSix?.click();
  const launchedClaude = await waitUntil(async () => {
    const snapshot = await api.listTerminals();
    const matches = snapshot.terminals.filter(terminal =>
      terminal.launch?.version === 1 && terminal.launch.profileId === '${PIN.claudeProfileId}');
    return matches.length === 6 && matches.every(terminal => terminal.state === 'running')
      ? matches.sort((left, right) => left.title.localeCompare(right.title)) : null;
  }, 'Six packaged Claude terminals did not reach running state.');
  reportProgress('six-terminals-running');
  await waitUntil(() => launchedClaude.every(terminal =>
    (terminalOutput.get(terminal.id) ?? '').includes('fake-claude-ready')),
  'Six packaged Claude terminals did not deliver initial PTY output.');
  const truncatedAgentTargets = launchedClaude.filter(terminal =>
    (terminalOutput.get(terminal.id) ?? '').includes('fake-claude-redraw-target'));
  const truncatedAgentTarget = truncatedAgentTargets[0] ?? null;
  const truncatedAgentInitialOutputBytes = truncatedAgentTarget === null ? 0
    : (terminalOutput.get(truncatedAgentTarget.id) ?? '').length;
  reportProgress('six-terminals-initial-output');
  reportProgress('six-terminal-ui-mounting');
  const preInputTerminalPaneCount = await waitFor(() => {
    const panes = document.querySelectorAll('.${PIN.terminalPane}').length;
    const views = document.querySelectorAll('.${PIN.terminalView} .${PIN.xterm}').length;
    reportProgress('six-terminal-ui-mounting', { panes, views });
    return panes === 6 && views === 6 ? panes : null;
  });
  if (preInputTerminalPaneCount !== 6) {
    throw new Error('Six packaged Claude terminals did not mount in the terminal grid: '
      + JSON.stringify(terminalDomState()));
  }
  const preInputReadyCount = await waitFor(() => {
    const count = document.querySelectorAll(
      '.${PIN.terminalPane} .${PIN.terminalView}[${PIN.terminalReadyAttr}]'
    ).length;
    reportProgress('six-terminal-ui-replay', { preInputReadyCount: count });
    return count === 6 ? count : null;
  });
  if (preInputReadyCount !== 6) {
    throw new Error('Six packaged Claude terminal views did not finish initial replay: '
      + JSON.stringify(terminalDomState()));
  }
  reportProgress('six-terminal-ui-ready');
  const plainClickCandidate = launchedClaude.map(terminal => {
    const viewId = terminal.id + ':' + String(terminal.generation);
    const view = document.querySelector(
      '[${PIN.terminalIdAttr}="' + CSS.escape(viewId) + '"]'
    );
    return { terminal, view, pane: view?.closest('.${PIN.terminalPane}') ?? null };
  }).find(candidate => candidate.terminal.id !== truncatedAgentTarget?.id
    && candidate.view !== null && candidate.pane !== null
    && !candidate.pane.classList.contains('${PIN.terminalPaneFocused}'));
  const plainClickScreen = plainClickCandidate?.view?.querySelector('.${PIN.xtermScreen}') ?? null;
  if (!(plainClickScreen instanceof HTMLElement) || plainClickCandidate?.pane === null
      || plainClickCandidate === undefined) {
    throw new Error('Packaged click-input probe could not find an unfocused normal terminal.');
  }
  api.sendTerminalInput(
    plainClickCandidate.terminal.id,
    plainClickCandidate.terminal.generation,
    new TextEncoder().encode('plain-click-prime\\n'),
  );
  const plainClickPrimed = await waitFor(() => (
    terminalOutput.get(plainClickCandidate.terminal.id) ?? ''
  ).includes('fake-claude-echo:plain-click-prime') ? true : null) === true;
  if (!plainClickPrimed) {
    throw new Error('Packaged click-input probe could not prime its normal terminal.');
  }
  const plainClickOutputOffset = (
    terminalOutput.get(plainClickCandidate.terminal.id) ?? ''
  ).length;
  const clickBounds = plainClickScreen.getBoundingClientRect();
  const clickInit = {
    bubbles: true, cancelable: true, composed: true, button: 0, detail: 1,
    clientX: clickBounds.right - 2, clientY: clickBounds.bottom - 2,
  };
  plainClickScreen.dispatchEvent(new PointerEvent('pointerdown', { ...clickInit, buttons: 1 }));
  plainClickScreen.dispatchEvent(new MouseEvent('mousedown', { ...clickInit, buttons: 1 }));
  plainClickScreen.dispatchEvent(new PointerEvent('pointerup', { ...clickInit, buttons: 0 }));
  plainClickScreen.dispatchEvent(new MouseEvent('mouseup', { ...clickInit, buttons: 0 }));
  const plainClickFocused = await waitFor(() =>
    plainClickCandidate.pane?.classList.contains('${PIN.terminalPaneFocused}') ? true : null) === true;
  // Let any xterm-generated click input cross the renderer/host boundary before
  // terminating the fixture's canonical input line with the sentinel.
  await new Promise(resolve => setTimeout(resolve, 100));
  api.sendTerminalInput(
    plainClickCandidate.terminal.id,
    plainClickCandidate.terminal.generation,
    new TextEncoder().encode('plain-click-sentinel\\n'),
  );
  const plainClickOutput = await waitFor(() => {
    const output = (terminalOutput.get(plainClickCandidate.terminal.id) ?? '')
      .slice(plainClickOutputOffset);
    return output.match(/fake-claude-echo:[^\\r\\n]*plain-click-sentinel/u)?.[0] ?? null;
  });
  const plainClickDidNotEmitInput = plainClickOutput
    === 'fake-claude-echo:plain-click-sentinel';
  reportProgress('plain-click-input-safe', { plainClickFocused, plainClickDidNotEmitInput });
  document.querySelector(
    '.${PIN.terminalPane} button[aria-label="${PIN.morePaneActions}"]'
  )?.click();
  const passOffAction = await waitFor(() => {
    const button = document.querySelector(
      '[role="menu"][aria-label="${PIN.paneActionsMenu}"] button[aria-label="${PIN.passOffAction}"]'
    );
    return button instanceof HTMLButtonElement && !button.disabled ? button : null;
  });
  passOffAction?.click();
  const passOffMenu = await waitFor(() => document.querySelector(
    '[role="menu"][aria-label="${PIN.passOffMenu}"]'
  ));
  const passOffProfileIds = passOffMenu === null ? [] : [...passOffMenu.querySelectorAll(
    'button[${PIN.passOffProfileAttr}]:not(:disabled)'
  )].map(button => button.getAttribute('${PIN.launchProfileAttr}'));
  passOffMenu?.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'Escape', bubbles: true, cancelable: true,
  }));
  launchedClaude.forEach((terminal, index) => {
    api.sendTerminalInput(terminal.id, terminal.generation,
      new TextEncoder().encode('probe-' + String(index + 1) + '\\n'));
  });
  await waitUntil(() => launchedClaude.every((terminal, index) =>
    (terminalOutput.get(terminal.id) ?? '').includes(
      'fake-claude-echo:probe-' + String(index + 1))),
  'Six packaged Claude terminals did not complete bidirectional PTY I/O.');
  reportProgress('six-terminals-echoed');
  const focusedPaneBeforeHotkey = document.querySelector('.${PIN.terminalPane}.${PIN.terminalPaneFocused}')
    ?.getAttribute('${PIN.terminalPaneIdAttr}') ?? null;
  const hotkeyTarget = document.querySelector(
    '.${PIN.terminalPane}.${PIN.terminalPaneFocused} .${PIN.xtermHelperTextarea}'
  );
  const hotkeyEvent = new KeyboardEvent('keydown', {
    key: ']', code: 'BracketRight', metaKey: true, shiftKey: true,
    bubbles: true, cancelable: true,
  });
  hotkeyTarget?.dispatchEvent(hotkeyEvent);
  const focusedPaneAfterHotkey = await waitFor(() => {
    const pane = document.querySelector('.${PIN.terminalPane}.${PIN.terminalPaneFocused}')
      ?.getAttribute('${PIN.terminalPaneIdAttr}') ?? null;
    return pane !== null && pane !== focusedPaneBeforeHotkey ? pane : null;
  });
  const hotkeySwitchedTerminal = focusedPaneBeforeHotkey !== null
    && focusedPaneAfterHotkey !== null && hotkeyEvent.defaultPrevented;

  // Settings lives below Workspaces in the persistent sidebar footer.
  const settingsChoice = await waitFor(() => document.querySelector('button[aria-label="${PIN.settingsButton}"]'));
  settingsChoice?.click();
  const settingsOpened = await waitFor(() =>
    document.querySelector('.${PIN.settingsOverlay}') === null ? null : true);
  const focusedPaneBeforeOverlayHotkey = document.querySelector('.${PIN.terminalPane}.${PIN.terminalPaneFocused}')
    ?.getAttribute('${PIN.terminalPaneIdAttr}') ?? null;
  const overlayHotkeyTarget = document.querySelector(
    '.${PIN.terminalPane}.${PIN.terminalPaneFocused} .${PIN.xtermHelperTextarea}'
  );
  overlayHotkeyTarget?.dispatchEvent(new KeyboardEvent('keydown', {
    key: ']', code: 'BracketRight', metaKey: true, shiftKey: true,
    bubbles: true, cancelable: true,
  }));
  await new Promise(resolve => setTimeout(resolve, 200));
  const focusedPaneAfterOverlayHotkey = document.querySelector('.${PIN.terminalPane}.${PIN.terminalPaneFocused}')
    ?.getAttribute('${PIN.terminalPaneIdAttr}') ?? null;
  const overlayHotkeyIgnored = settingsOpened === true
    && focusedPaneAfterOverlayHotkey === focusedPaneBeforeOverlayHotkey;
  document.querySelector('button[aria-label="${PIN.closeSettingsButton}"]')?.click();
  await waitFor(() =>
    document.querySelector('.${PIN.settingsOverlay}') === null ? true : null);
  reportProgress('settings-closed');

  const renderedTerminalEchoCount = () => [...document.querySelectorAll('.${PIN.terminalPane}')]
    .filter(pane => /fake-claude-echo:probe-[1-6]/u.test(
      pane.querySelector('.${PIN.xtermAccessibilityTree}')?.textContent ?? '')).length;
  const restoredTerminalCount = () => document.querySelectorAll(
    '.${PIN.terminalPane} .${PIN.terminalView}[${PIN.terminalReadyAttr}]'
  ).length;
  const initialRenderedTerminalEchoCount = await waitFor(() => {
    const count = renderedTerminalEchoCount();
    reportProgress('initial-terminal-echo', {
      echoCount: count,
      restoredCount: restoredTerminalCount(),
    });
    return count === 6 ? count : null;
  });
  await waitFor(() => {
    const count = restoredTerminalCount();
    reportProgress('initial-terminal-ready', {
      echoCount: renderedTerminalEchoCount(),
      restoredCount: count,
    });
    return count === 6 ? true : null;
  });
  reportProgress('initial-terminal-replay-ready');
  // Flip lives in the window's ··· menu, which closes after each choice.
  const flipTerminals = async () => {
    document.querySelector('button[aria-label="${PIN.moreWorkspaceOptions}"]')?.click();
    const row = await waitFor(() => [...document.querySelectorAll('[role="menuitemcheckbox"]')]
      .find(item => item.textContent.trim() === '${PIN.flipTerminals}'));
    row?.click();
  };
  reportProgress('cards-opening');
  await flipTerminals();
  const cardsCoveredPaneCount = await waitFor(() => {
    const cardCount = document.querySelectorAll('.${PIN.terminalPaneCard}').length;
    const mountedPaneCount = document.querySelectorAll('.${PIN.terminalPane} .${PIN.xterm}').length;
    return cardCount === 6 && mountedPaneCount === 6 ? mountedPaneCount : null;
  });
  reportProgress('cards-opened', { cardsCoveredPaneCount });
  await flipTerminals();
  reportProgress('cards-returning');
  const cardsReturnEchoCount = await waitFor(() => {
    if (document.querySelector('.${PIN.terminalPaneCard}') !== null) return null;
    const count = renderedTerminalEchoCount();
    return count === 6 ? count : null;
  });
  reportProgress('cards-round-trip-complete', { cardsReturnEchoCount });

  const activeWorkspaceButton = () => document.querySelector(
    '[${PIN.workspaceIdAttr}][${PIN.activeWorkspaceAttr}="page"]'
  );
  const originalWorkspaceId = activeWorkspaceButton()
    ?.getAttribute('${PIN.workspaceIdAttr}') ?? null;
  let workspaceRoundTripCount = 0;
  let truncatedAgentPreservedRoundTripCount = 0;
  const workspaceAttemptDiagnostics = [];
  if (originalWorkspaceId !== null) {
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const retainedViews = launchedClaude.map(terminal => {
        const terminalViewId = terminal.id + ':' + String(terminal.generation);
        return document.querySelector(
          '[${PIN.terminalIdAttr}="' + CSS.escape(terminalViewId) + '"]'
        )?.querySelector('.${PIN.xterm}') ?? null;
      });
      const targetIndex = launchedClaude.findIndex(terminal =>
        terminal.id === truncatedAgentTarget?.id);
      const targetText = retainedViews[targetIndex]
        ?.querySelector('.${PIN.xtermAccessibilityTree}')?.textContent ?? '';
      const retainedFrame = targetText.match(/fake-claude-frame:[0-9]+x[0-9]+:[0-9]+/u)?.[0] ?? null;
      reportProgress('workspace-new-button', { attempt: attempt + 1 });
      const newWorkspaceButton = await waitFor(() => {
        const button = document.querySelector(
          '.${PIN.sidebarPrimaryNav} button[aria-label="${PIN.newWorkspace}"]'
        );
        const originalButton = [...document.querySelectorAll(
          '[${PIN.workspaceIdAttr}]'
        )].find(candidate => candidate.getAttribute('${PIN.workspaceIdAttr}')
          === originalWorkspaceId) ?? null;
        return button !== null && originalButton !== null && !originalButton.disabled
          ? button : null;
      });
      newWorkspaceButton?.click();
      const newWorkspaceMenuItem = await waitFor(() => {
        const menu = document.querySelector(
          '[role="menu"][aria-label="${PIN.workspaceCreateMenu}"]'
        );
        return [...(menu?.querySelectorAll('[role="menuitem"]') ?? [])]
          .find(item => item.textContent.trim() === '${PIN.newWorkspaceMenuItem}'
            && !item.disabled) ?? null;
      });
      if (newWorkspaceMenuItem === null) {
        throw new Error('Workspaces plus menu did not offer New workspace');
      }
      newWorkspaceMenuItem.click();
      reportProgress('workspace-created-id', { attempt: attempt + 1 });
      const createdWorkspaceId = await waitFor(() => {
        const activeId = window.localStorage.getItem(
          '${PIN.workspaceStorageKey}'
        );
        return activeId !== null && activeId !== originalWorkspaceId ? activeId : null;
      });
      reportProgress('workspace-created-visible', { attempt: attempt + 1, createdWorkspaceId });
      const createdWorkspaceVisible = await waitFor(() =>
        createdWorkspaceId !== null
          && document.querySelector('.${PIN.terminalLauncher}') !== null ? true : null);
      const returnWorkspaceButton = [...document.querySelectorAll(
        '[${PIN.workspaceIdAttr}]'
        )].find(button => button.getAttribute('${PIN.workspaceIdAttr}')
          === originalWorkspaceId) ?? null;
      returnWorkspaceButton?.click();
      reportProgress('workspace-returning', {
        attempt: attempt + 1,
        createdWorkspaceId,
        returnWorkspaceButtonFound: returnWorkspaceButton !== null,
      });
      const returned = await waitFor(() =>
        window.localStorage.getItem('${PIN.workspaceStorageKey}')
          === originalWorkspaceId
          && activeWorkspaceButton()?.getAttribute('${PIN.workspaceIdAttr}')
            === originalWorkspaceId ? true : null);
      const attemptDiagnostic = {
        attempt: attempt + 1,
        newWorkspaceButtonFound: newWorkspaceButton !== null,
        newWorkspaceMenuItemFound: newWorkspaceMenuItem !== null,
        createdWorkspaceId,
        createdWorkspaceVisible,
        returnWorkspaceButtonFound: returnWorkspaceButton !== null,
        returned,
        echoCount: null,
        restoredCount: null,
        retainedViewCount: null,
        retainedFrame,
        retainedFrameRendered: null,
        liveEchoCount: null,
      };
      if (createdWorkspaceId === null || createdWorkspaceVisible !== true || returned !== true) {
        workspaceAttemptDiagnostics.push(attemptDiagnostic);
        break;
      }
      reportProgress('workspace-waiting-for-echo', { attempt: attempt + 1 });
      const echoCount = await waitFor(() => {
        const count = renderedTerminalEchoCount();
        reportProgress('workspace-waiting-for-echo', {
          attempt: attempt + 1,
          echoCount: count,
          restoredCount: restoredTerminalCount(),
        });
        return count === 6 ? count : null;
      });
      reportProgress('workspace-waiting-for-ready', { attempt: attempt + 1, echoCount });
      const restoredCount = await waitFor(() => {
        const count = restoredTerminalCount();
        reportProgress('workspace-waiting-for-ready', {
          attempt: attempt + 1,
          echoCount: renderedTerminalEchoCount(),
          restoredCount: count,
        });
        return count === 6 ? count : null;
      });
      const retainedViewCount = launchedClaude.filter((terminal, index) => {
        const terminalViewId = terminal.id + ':' + String(terminal.generation);
        const current = document.querySelector(
          '[${PIN.terminalIdAttr}="' + CSS.escape(terminalViewId) + '"]'
        )?.querySelector('.${PIN.xterm}') ?? null;
        return current !== null && current === retainedViews[index];
      }).length;
      const retainedFrameRendered = retainedFrame !== null
        && (retainedViews[targetIndex]?.querySelector('.${PIN.xtermAccessibilityTree}')
          ?.textContent ?? '').includes(retainedFrame);
      // Prove the retained emulators still parse new PTY output after returning.
      // This probe leaves the original echo available for the screen checks.
      const liveProbe = '__IFACE_PRESERVED_PROBE_' + String(attempt + 1) + '__';
      launchedClaude.forEach(terminal => api.sendTerminalInput(terminal.id,
        terminal.generation, new TextEncoder().encode(liveProbe + '\\n')));
      const liveEchoCount = await waitFor(() => {
        const count = retainedViews.filter(view =>
          (view?.querySelector('.${PIN.xtermAccessibilityTree}')?.textContent ?? '')
            .includes('fake-claude-live:' + liveProbe)).length;
        return count === 6 ? count : null;
      });
      attemptDiagnostic.echoCount = echoCount;
      attemptDiagnostic.restoredCount = restoredCount;
      attemptDiagnostic.retainedViewCount = retainedViewCount;
      attemptDiagnostic.retainedFrameRendered = retainedFrameRendered;
      attemptDiagnostic.liveEchoCount = liveEchoCount;
      workspaceAttemptDiagnostics.push(attemptDiagnostic);
      if (echoCount !== 6 || restoredCount !== 6 || retainedViewCount !== 6
          || retainedFrameRendered !== true || liveEchoCount !== 6) break;
      workspaceRoundTripCount += 1;
      truncatedAgentPreservedRoundTripCount += 1;
      reportProgress('workspace-round-trip-complete', {
        attempt: attempt + 1,
        workspaceRoundTripCount,
        truncatedAgentPreservedRoundTripCount,
      });
    }
  }
  reportProgress('workspace-final-echo', { workspaceRoundTripCount, workspaceAttemptDiagnostics });
  const workspaceReturnEchoCount = await waitFor(() => {
    const count = renderedTerminalEchoCount();
    reportProgress('workspace-final-echo', {
      workspaceRoundTripCount,
      workspaceAttemptDiagnostics,
      echoCount: count,
      restoredCount: restoredTerminalCount(),
    });
    return count === 6 ? count : null;
  });
  launchedClaude.forEach(terminal => {
    api.sendTerminalInput(terminal.id, terminal.generation,
      new TextEncoder().encode('__IFACE_FAKE_CLAUDE_EXIT__\\n'));
  });
  // An agent CLI exiting cleanly hands its pane to a shell in its cwd: the
  // renderer dismisses the exited Claude terminal once its replacement shell
  // exists, so a clean exit ends with the Claude id gone and a running
  // shell-profile terminal in its place. A non-zero exit or a stall would
  // leave the Claude terminal in the snapshot and fail this wait.
  await waitUntil(async () => {
    const snapshot = await api.listTerminals();
    const claudeGone = launchedClaude.every(terminal =>
      !snapshot.terminals.some(candidate => candidate.id === terminal.id));
    const handoffShells = snapshot.terminals.filter(candidate =>
      candidate.launch?.version === 1 && candidate.launch.profileId === '${PIN.shellProfileId}'
      && candidate.state === 'running'
      && launchedClaude.some(terminal => terminal.cwd === candidate.cwd));
    return claudeGone && handoffShells.length >= 6;
  }, 'Six exited Claude terminals were not handed off to shells.', { bridge: false });
  reportProgress('six-terminals-exited');
  const terminalPanes = [...document.querySelectorAll('.${PIN.terminalPane}')];
  const paneRects = terminalPanes.map(pane => {
    const bounds = pane.getBoundingClientRect();
    return { left: bounds.left, top: bounds.top, width: bounds.width, height: bounds.height };
  });
  // getComputedStyle() is live. Snapshot primitives before the removal below
  // so disconnecting one pane cannot erase the six-pane geometry we measured.
  const paneFocusedStates = terminalPanes.map(pane =>
    pane.classList.contains('${PIN.terminalPaneFocused}'));
  const paneHeaderHeights = terminalPanes.map(pane =>
    pane.querySelector('.${PIN.terminalPaneHead}')?.getBoundingClientRect().height ?? 0);
  const axisCount = axis => {
    const values = [];
    for (const rect of paneRects) {
      const value = rect[axis];
      if (!values.some(existing => Math.abs(existing - value) <= 1)) values.push(value);
    }
    return values.length;
  };
  // Exercise the user's actual removal path in Chromium, after preserving the
  // six-pane measurements above. Closing one pane must reflow 6 -> 5 as a
  // full-width 3 + 2 grid instead of leaving the sixth cell empty.
  // One noun per object: the header's right-hand × closes the ACTIVE TERMINAL
  // and says so, which is what tells it apart from the per-tab × beside it.
  const closeForReflow = terminalPanes[3]?.querySelector(
    '.${PIN.terminalPaneAction}[aria-label="${PIN.closeActiveTerminal}"]');
  if (!(closeForReflow instanceof HTMLButtonElement)) {
    throw new Error(
      'The packaged terminal pane did not expose its close-active-terminal action.');
  }
  closeForReflow.click();
  const terminalRemovalRows = await waitFor(() => {
    const remaining = [...document.querySelectorAll('.${PIN.terminalPane}')];
    if (remaining.length !== 5) return null;
    const rows = [];
    for (const pane of remaining) {
      const bounds = pane.getBoundingClientRect();
      let row = rows.find(candidate => Math.abs(candidate.top - bounds.top) <= 1);
      if (row === undefined) {
        row = { top: bounds.top, rects: [] };
        rows.push(row);
      }
      row.rects.push({ left: bounds.left, right: bounds.right, width: bounds.width });
    }
    rows.sort((left, right) => left.top - right.top);
    for (const row of rows) row.rects.sort((left, right) => left.left - right.left);
    if (rows.length !== 2 || rows[0].rects.length !== 3 || rows[1].rects.length !== 2) {
      return null;
    }
    // The exit ghost disappears before the surviving panes finish resizing.
    // Wait for the same complete geometry the validator requires, rather than
    // accepting the first animation frame that happens to contain five panes.
    const [upper, lower] = rows.map(row => row.rects);
    const equalWidths = rects => Math.max(...rects.map(rect => rect.width))
      - Math.min(...rects.map(rect => rect.width)) <= 1;
    if (!equalWidths(upper) || !equalWidths(lower)
        || Math.abs(upper[0].left - lower[0].left) > 1
        || Math.abs(upper[2].right - lower[1].right) > 1
        || lower[0].width <= upper[0].width) return null;
    return rows.map(row => ({
      count: row.rects.length,
      left: row.rects[0].left,
      right: row.rects[row.rects.length - 1].right,
      widths: row.rects.map(rect => rect.width),
    }));
  });
  if (terminalRemovalRows === null) {
    throw new Error('Removing one packaged terminal did not produce a five-pane 3 + 2 grid.');
  }
  removeTerminalData();
  removeTerminalFailure();
  terminalMutationObserver.disconnect();
  reportProgress('complete');
  return {
    title: document.title,
    viewport: { width: innerWidth, height: innerHeight },
    readyState: document.readyState,
    hasRoot: document.querySelector('#${PIN.rootId}') !== null,
    hasIface: typeof api === 'object' && typeof api.listSessions === 'function'
      && typeof api.getDefaultDirectory === 'function'
      && typeof api.listProviderCommands === 'function'
      && typeof api.listTerminalLaunchProfiles === 'function'
      && typeof api.startAdversarialReview === 'function'
      && typeof api.getUpdateState === 'function',
    adversarialReviewGuarded,
    sessionCount: Array.isArray(sessions) ? sessions.length : -1,
    defaultDirectory,
    updatePhase: updateState?.phase ?? null,
    repositoryName,
    providerLogo,
    modelSelectorLabel,
    commandButtonLabel,
    commandButtonWidth,
    surfaceSwitchLabels,
    usageEngineIds,
    geminiUsageWindows,
    launchProfileIds: Array.isArray(launchProfiles)
      ? launchProfiles.map(profile => profile.id) : [],
    passOffProfileIds,
    launchProfileFieldsSafe: Array.isArray(launchProfiles) && launchProfiles.every(profile =>
      profile !== null && typeof profile === 'object' && Object.keys(profile).every(key =>
        ['id', 'engine', 'label', 'available', 'unavailableReason', 'installHint', 'version']
          .includes(key))),
    launcherProfiles,
    launcherCounts,
    launcherLogos,
    launcherSectionLabels,
    interfaceMarkCount,
    launcherSingleActionLabel,
    launcherReviewLineup,
    launcherReviewActionLabel,
    launcherReviewNeedsObjective,
    launcherReviewCanStart,
    launcherReviewDismissed,
    launcherSixCountChecked,
    launcherActionLabel,
    launcherCwd: terminalLauncher?.querySelector('.${PIN.launcherCwdPath}')
      ?.getAttribute('title') ?? null,
    claudeBatchCount: launchedClaude.length,
    preInputTerminalPaneCount,
    preInputReadyCount,
    plainClickFocused,
    plainClickDidNotEmitInput,
    claudeBatchEchoCount: launchedClaude.filter((terminal, index) =>
      (terminalOutput.get(terminal.id) ?? '').includes(
        'fake-claude-echo:probe-' + String(index + 1))).length,
    hotkeySwitchedTerminal,
    overlayHotkeyIgnored,
    initialRenderedTerminalEchoCount,
    cardsCoveredPaneCount,
    cardsReturnEchoCount,
    workspaceCreated: workspaceRoundTripCount === 3,
    workspaceRoundTripCount,
    workspaceAttemptDiagnostics,
    workspaceReturnEchoCount,
    truncatedAgentTargetCount: truncatedAgentTargets.length,
    truncatedAgentInitialOutputBytes,
    truncatedAgentPreservedRoundTripCount,
    terminalDataFailures: terminalFailures,
    terminalPaneCount: paneRects.length,
    terminalPaneWidths: paneRects.map(rect => rect.width),
    terminalPaneHeights: paneRects.map(rect => rect.height),
    terminalGridColumns: axisCount('left'),
    terminalGridRows: axisCount('top'),
    terminalRemovalRows,
    terminalPaneFocusedStates: paneFocusedStates,
    terminalPaneHeaderHeights: paneHeaderHeights
  };
})()
`;

export function validatePackagedRendererProbe(value, errors = [], { requireUpdater = false } = {}) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail("Packaged renderer returned an invalid smoke result.");
  }
  // A packaged build with a public channel must come up with updates on: the
  // only way it reads "disabled" is a signature the updater refuses, and
  // then no installed copy could ever update again. (An unpackaged Dev app
  // is disabled by design and never runs this smoke.)
  if (requireUpdater && (typeof value.settledUpdatePhase !== "string"
      || value.settledUpdatePhase === "disabled")) {
    fail("The packaged updater is off although this build follows the public update channel; "
      + "the app's signature is one the updater cannot read.");
  }
  if (value.title !== PIN.documentTitle) fail("Packaged renderer has the wrong document title.");
  if (value.viewport?.width !== PACKAGED_RENDERER_VIEWPORT.width
      || value.viewport?.height !== PACKAGED_RENDERER_VIEWPORT.height) {
    fail("Packaged renderer did not use the pinned desktop viewport.");
  }
  if (value.readyState !== "complete" && value.readyState !== "interactive") {
    fail("Packaged renderer never completed document loading.");
  }
  if (value.hasRoot !== true) fail("Packaged renderer did not mount its root element.");
  if (value.hasIface !== true) fail("Packaged preload did not expose the Interface API.");
  if (!Number.isInteger(value.sessionCount) || value.sessionCount < 0) {
    fail("Packaged renderer could not invoke the session-list IPC bridge.");
  }
  if (typeof value.defaultDirectory !== "string" || !value.defaultDirectory.startsWith("/")) {
    fail("Packaged renderer could not resolve its default directory.");
  }
  if (typeof value.updatePhase !== "string" || value.updatePhase.length === 0) {
    fail("Packaged renderer could not invoke the updater-state IPC bridge.");
  }
  const expectedRepository = value.defaultDirectory.split(/[\\/]/).filter(Boolean).at(-1);
  if (value.repositoryName !== expectedRepository) {
    fail("Packaged composer did not display its resolved default directory.");
  }
  if (value.providerLogo !== PIN.claudeLogo) {
    fail("Packaged composer did not display its default provider logo.");
  }
  const modelSelectorLabel = typeof value.modelSelectorLabel === "string"
    ? value.modelSelectorLabel.toLowerCase() : null;
  if (modelSelectorLabel === null
      || !modelSelectorLabel.startsWith(`${PIN.composerModelLabel.toLowerCase()}:`)
      || modelSelectorLabel.includes(PIN.composerModelPlaceholder.toLowerCase())) {
    fail("Packaged composer did not select a concrete model for its command probe.");
  }
  if (typeof value.commandButtonLabel !== "string"
      || !value.commandButtonLabel.toLowerCase()
        .includes(PIN.composerCommandsFor.toLowerCase())
      || typeof value.commandButtonWidth !== "number" || value.commandButtonWidth < 24) {
    fail(`Packaged composer did not display model-scoped provider commands (label: ${
      String(value.commandButtonLabel)}, width: ${String(value.commandButtonWidth)}).`);
  }
  if (JSON.stringify(value.surfaceSwitchLabels)
        !== JSON.stringify([PIN.surfaceAgent, PIN.surfaceCode, PIN.surfaceChat])) {
    fail("Packaged workspace did not expose the exact Threads, Code, and Agent surface switch.");
  }
  if (!Array.isArray(value.usageEngineIds)
      || value.usageEngineIds.includes("acp:qwen")
      || !value.usageEngineIds.includes(PIN.geminiEngineId)
      || JSON.stringify(value.geminiUsageWindows)
        !== JSON.stringify([
          { label: PIN.geminiFiveHourWindow, percent: usageWindowPercent(60) },
          { label: PIN.geminiWeeklyWindow, percent: usageWindowPercent(25) },
        ])) {
    fail("Packaged Usage did not omit Qwen or render Antigravity Gemini quota.");
  }
  if (JSON.stringify(value.launchProfileIds)
      !== JSON.stringify(PACKAGED_LAUNCH_PROFILE_IDS)
      || value.launchProfileFieldsSafe !== true) {
    fail("Packaged Rust host did not expose the safe terminal launch-profile catalog.");
  }
  if (JSON.stringify(value.passOffProfileIds)
      !== JSON.stringify(PACKAGED_PASS_OFF_PROFILE_IDS)) {
    fail("Packaged terminal Pass off did not offer the complete agent roster.");
  }
  if (JSON.stringify(value.launcherProfiles)
      !== JSON.stringify([...PACKAGED_AGENT_PROFILE_IDS, PIN.shellProfileId])
      || JSON.stringify(value.launcherLogos)
      !== JSON.stringify([PIN.claudeLogo, PIN.codexLogo, PIN.grokLogo,
        PIN.antigravityLogo, PIN.ompLogo, PIN.opencodeLogo, PIN.museLogo, PIN.kimiLogo])) {
    fail("Packaged terminal launcher did not display the owner's agent roster.");
  }
  if (JSON.stringify(value.launcherSectionLabels)
      !== JSON.stringify([PIN.launcherAgentHeading, PIN.launcherCountHeading,
        PIN.launcherIsolationHeading, PIN.launcherCwdHeading, PIN.launcherPreviewHeading])) {
    fail("Packaged terminal launcher did not preserve the reviewed session-composer structure.");
  }
  // The launcher retains the Fold; the quiet header uses only the wordmark.
  if (!Number.isInteger(value.interfaceMarkCount) || value.interfaceMarkCount !== 1) {
    fail("Packaged renderer did not replace its literal IF placeholders with the Fold mark.");
  }
  if (value.launcherSingleActionLabel !== launchButtonLabel(1, PIN.claudeCodeLabel)) {
    fail("Packaged terminal launcher did not name its opening selection on the launch button.");
  }
  if (value.launcherReviewLineup !== PIN.reviewLineupText
      || value.launcherReviewActionLabel !== PIN.reviewStart
      || value.launcherReviewNeedsObjective !== true
      || value.launcherReviewCanStart !== true
      || value.launcherReviewDismissed !== true
      || value.adversarialReviewGuarded !== true) {
    fail("Packaged review dialog did not expose the objective-gated builder and reviewer workflow.");
  }
  if (value.launcherSixCountChecked !== true
      || value.launcherActionLabel !== launchButtonLabel(6, PIN.claudeCodeLabel)) {
    fail("Packaged terminal launcher did not apply the six-terminal selection.");
  }
  if (JSON.stringify(value.launcherCounts)
      !== JSON.stringify(Array.from({ length: Number(PIN.launcherCountLimit) }, (_, index) => index + 1))) {
    fail("Packaged terminal launcher did not display exact count buttons 1 through 8.");
  }
  if (value.launcherCwd !== value.defaultDirectory) {
    fail("Packaged terminal launcher did not display its approved working directory.");
  }
  if (value.claudeBatchCount !== 6 || value.preInputTerminalPaneCount !== 6
      || value.preInputReadyCount !== 6 || value.claudeBatchEchoCount !== 6
      || value.terminalDataFailures?.length !== 0) {
    const diagnostic = JSON.stringify({
      batch: value.claudeBatchCount,
      preInputPanes: value.preInputTerminalPaneCount,
      preInputReady: value.preInputReadyCount,
      echoes: value.claudeBatchEchoCount,
      panes: value.terminalPaneCount,
      failures: value.terminalDataFailures,
    });
    fail(`Packaged six-Claude terminal batch did not complete its PTY data round-trip: ${diagnostic}`);
  }
  if (value.plainClickFocused !== true || value.plainClickDidNotEmitInput !== true) {
    fail("Packaged terminal plain click did not focus without injecting PTY input.");
  }
  if (value.hotkeySwitchedTerminal !== true || value.overlayHotkeyIgnored !== true) {
    fail("Packaged terminal hotkey did not switch panes or stay inert behind Settings.");
  }
  if (value.initialRenderedTerminalEchoCount !== 6
      || value.cardsCoveredPaneCount !== 6
      || value.cardsReturnEchoCount !== 6
      || value.workspaceCreated !== true
      || value.workspaceRoundTripCount !== 3
      || value.workspaceReturnEchoCount !== 6
      || value.terminalPaneCount !== 6) {
    const diagnostic = JSON.stringify({
      initialRenderedTerminalEchoCount: value.initialRenderedTerminalEchoCount,
      cardsCoveredPaneCount: value.cardsCoveredPaneCount,
      cardsReturnEchoCount: value.cardsReturnEchoCount,
      workspaceCreated: value.workspaceCreated,
      workspaceRoundTripCount: value.workspaceRoundTripCount,
      workspaceAttemptDiagnostics: value.workspaceAttemptDiagnostics,
      workspaceReturnEchoCount: value.workspaceReturnEchoCount,
      terminalPaneCount: value.terminalPaneCount,
    });
    fail(`Packaged terminal output did not survive card-flip and workspace transitions: ${diagnostic}`);
  }
  if (value.truncatedAgentTargetCount !== 1
      || !Number.isInteger(value.truncatedAgentInitialOutputBytes)
      || value.truncatedAgentInitialOutputBytes <= 1024 * 1024
      || value.truncatedAgentPreservedRoundTripCount !== 3
      || !Array.isArray(value.workspaceAttemptDiagnostics)
      || value.workspaceAttemptDiagnostics.length !== 3
      || value.workspaceAttemptDiagnostics.some(attempt => attempt?.retainedViewCount !== 6
        || attempt.retainedFrameRendered !== true || attempt.liveEchoCount !== 6)) {
    const diagnostic = JSON.stringify({
      targetCount: value.truncatedAgentTargetCount,
      initialOutputBytes: value.truncatedAgentInitialOutputBytes,
      preservedRoundTrips: value.truncatedAgentPreservedRoundTripCount,
      workspaceAttemptDiagnostics: value.workspaceAttemptDiagnostics,
    });
    fail(`Packaged truncated-agent preservation did not retain its emulator, frame and live input: ${diagnostic}`);
  }
  if (value.terminalGridColumns !== 3 || value.terminalGridRows !== 2) {
    fail("Packaged six-terminal batch did not render as a 3 by 2 grid.");
  }
  const dimensionSpread = dimensions => Math.max(...dimensions) - Math.min(...dimensions);
  if (!Array.isArray(value.terminalPaneWidths) || !Array.isArray(value.terminalPaneHeights)
      || value.terminalPaneWidths.length !== 6 || value.terminalPaneHeights.length !== 6
      || dimensionSpread(value.terminalPaneWidths) > 1
      || dimensionSpread(value.terminalPaneHeights) > 1) {
    fail("Packaged six-terminal batch did not give every pane equal dimensions.");
  }
  const removalRows = value.terminalRemovalRows;
  if (!Array.isArray(removalRows) || removalRows.length !== 2
      || removalRows[0]?.count !== 3 || removalRows[1]?.count !== 2
      || !Array.isArray(removalRows[0]?.widths) || removalRows[0].widths.length !== 3
      || !Array.isArray(removalRows[1]?.widths) || removalRows[1].widths.length !== 2
      || dimensionSpread(removalRows[0].widths) > 1
      || dimensionSpread(removalRows[1].widths) > 1
      || Math.abs(removalRows[0].left - removalRows[1].left) > 1
      || Math.abs(removalRows[0].right - removalRows[1].right) > 1
      || removalRows[1].widths[0] <= removalRows[0].widths[0]) {
    fail("Packaged terminal removal did not resize five panes into a full-width 3 + 2 grid: "
      + JSON.stringify(removalRows));
  }
  // Look values (gutters, corners, head height, borders, shadows, the
  // launcher's columns, the selector's width, the toolbar's centring) are the
  // renderer DOM gate's to check, in the same run as the CSS that sets them
  // (review F2, 2026-10-15). The packaged app keeps the facts packaging can
  // break: one focused tile, and heads that agree with each other.
  if (!Array.isArray(value.terminalPaneFocusedStates)
      || !Array.isArray(value.terminalPaneHeaderHeights)
      || value.terminalPaneFocusedStates.length !== 6
      || value.terminalPaneHeaderHeights.length !== 6
      || value.terminalPaneFocusedStates.some(focused => typeof focused !== "boolean")
      || value.terminalPaneFocusedStates.filter(focused => focused).length !== 1
      || value.terminalPaneHeaderHeights.some(height => typeof height !== "number" || height <= 0)
      || dimensionSpread(value.terminalPaneHeaderHeights) > 1) {
    const diagnostic = JSON.stringify({
      focusedStates: value.terminalPaneFocusedStates,
      headerHeights: value.terminalPaneHeaderHeights,
    });
    fail("Packaged terminal tiles did not keep one focused tile and equal heads: " + diagnostic);
  }
  if (errors.length > 0) fail(`Packaged renderer raised ${errors.length} fatal error(s).`);
  return value;
}

async function inspectRenderer(webSocketUrl, timeoutMs, WebSocketImpl, { requireUpdater = false } = {}) {
  const client = new CdpClient(webSocketUrl, WebSocketImpl);
  try {
    await deadline(client.ready, timeoutMs, "Packaged renderer connection");
    await deadline(client.call("Runtime.enable"), timeoutMs, "Runtime instrumentation");
    await deadline(client.call("Log.enable"), timeoutMs, "Renderer log instrumentation");
    await deadline(client.call("Emulation.setDeviceMetricsOverride", {
      ...PACKAGED_RENDERER_VIEWPORT, deviceScaleFactor: 1, mobile: false,
    }), timeoutMs, "Desktop smoke viewport");
    let response;
    try {
      response = await deadline(client.call("Runtime.evaluate", {
        expression: PACKAGED_RENDERER_PROBE,
        awaitPromise: true,
        returnByValue: true,
      }), timeoutMs, "Packaged renderer probe");
    } catch (error) {
      if (error instanceof Error && error.message === "Packaged renderer probe timed out.") {
        try {
          const progress = await deadline(client.call("Runtime.evaluate", {
            expression: "globalThis.__interfacePackagedSmokeProgress ?? null",
            returnByValue: true,
          }), 2_000, "Packaged renderer progress probe");
          fail(`${error.message} Last progress: ${JSON.stringify(progress?.result?.value ?? null)}`);
        } catch (progressError) {
          if (progressError instanceof Error
              && progressError.message.startsWith("Packaged renderer probe timed out.")) {
            throw progressError;
          }
        }
      }
      throw error;
    }
    if (response?.exceptionDetails !== undefined) {
      const detail = response.exceptionDetails.exception?.description
        ?? response.exceptionDetails.text ?? "unknown renderer exception";
      fail(`Packaged renderer probe threw an exception: ${String(detail)}`);
    }
    await new Promise(resolve => setTimeout(resolve, 250));
    const value = response?.result?.value;
    if (requireUpdater && value !== null && typeof value === "object") {
      const updater = await deadline(client.call("Runtime.evaluate", {
        expression: PACKAGED_UPDATER_PROBE,
        awaitPromise: true,
        returnByValue: true,
      }), UPDATER_SETTLE_MS + timeoutMs, "Packaged updater probe");
      value.settledUpdatePhase = updater?.result?.value ?? null;
    }
    return validatePackagedRendererProbe(value, client.errors, { requireUpdater });
  } finally {
    client.close();
  }
}

async function assertReleaseHostSupportsPrivateSmokeRoot(appPath) {
  const archive = await readFile(path.join(appPath, "Contents", "Resources", "app.asar"));
  if (!archive.includes(Buffer.from("iface-packaged-smoke-isolation-v1", "utf8"))) {
    fail("The packaged app does not support hidden, isolated production smoke tests.");
  }
  const host = path.join(appPath, "Contents", "Resources", "bin", "interface-host");
  const bytes = await readFile(host);
  if (!bytes.includes(Buffer.from(SMOKE_ROOT_MARKER, "utf8"))) {
    fail("The packaged Rust host does not support a private release-smoke data root.");
  }
}

async function stopChild(child) {
  if (child.exitCode !== null || child.pid === undefined) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise(resolve => child.once("exit", resolve)),
    new Promise(resolve => setTimeout(resolve, 2_000)),
  ]);
  if (child.exitCode === null) {
    child.kill("SIGKILL");
    await Promise.race([
      new Promise(resolve => child.once("exit", resolve)),
      new Promise(resolve => setTimeout(resolve, 2_000)),
    ]);
  }
}

/** Where one smoke profile keeps its data, homes and fake provider CLIs. */
function packagedSmokePaths(profile) {
  const fakeHome = path.join(profile, "home");
  return {
    dataRoot: path.join(profile, "data"),
    fakeHome,
    defaultDirectory: path.join(fakeHome, "Desktop", "Projects"),
    fakeClaude: path.join(profile, "fake-claude"),
    fakeClaudeRedrawClaim: path.join(profile, "fake-claude-redraw-claim"),
    fakeAntigravity: path.join(fakeHome, ".local", "bin", "agy"),
    fakeGrok: path.join(profile, "fake-grok"),
    fakeQwen: path.join(profile, "fake-qwen"),
    fakeQwenMlxServer: path.join(profile, "fake-qwen-mlx-server"),
  };
}

/**
 * The packaged app's executable, arguments and spawn options for one smoke
 * profile: the sandbox session environment, the private data root and home the
 * release host accepts only for a smoke, and the fake provider CLIs.
 */
export function packagedSmokeLaunch(appPath, profile, port) {
  const {
    dataRoot, fakeHome, fakeClaude, fakeClaudeRedrawClaim, fakeAntigravity, fakeGrok, fakeQwen,
    fakeQwenMlxServer,
  } = packagedSmokePaths(profile);
  return {
    executable: path.join(appPath, "Contents", "MacOS", "Interface"),
    args: [
      `--user-data-dir=${profile}`,
      "--remote-debugging-address=127.0.0.1",
      `--remote-debugging-port=${port}`,
      // The smoke interrogates rendered/accessibility state without taking over
      // the user's foreground app. Chromium otherwise throttles occluded test
      // windows and can defer accessibility refreshes after xterm parsed replay.
      "--disable-background-timer-throttling",
      "--disable-backgrounding-occluded-windows",
      "--disable-renderer-backgrounding",
    ],
    options: {
      env: {
        ...packagedSandboxEnvironment(profile),
        INTERFACE_HOST_SMOKE_DATA_ROOT: dataRoot,
        INTERFACE_HOST_SMOKE_HOME: fakeHome,
        INTERFACE_PACKAGED_SMOKE_REDRAW_CLAIM: fakeClaudeRedrawClaim,
        CLAUDE_CODE_EXECUTABLE: fakeClaude,
        CODEX_EXECUTABLE: fakeClaude,
        ANTIGRAVITY_EXECUTABLE: fakeAntigravity,
        GROK_EXECUTABLE: fakeGrok,
        QWEN_EXECUTABLE: fakeQwen,
        QWEN_MLX_SERVER_EXECUTABLE: fakeQwenMlxServer,
      },
      stdio: ["ignore", "pipe", "pipe"],
      shell: false,
      cwd: fakeHome,
    },
  };
}

/** Launch and interrogate an exact signed app extracted from the release ZIP. */
export async function smokePackagedApp({
  appPath,
  timeoutMs = DEFAULT_TIMEOUT_MS,
  spawnImpl = spawn,
  fetchImpl = fetch,
  WebSocketImpl = WebSocket,
  reservePort = reserveLoopbackPort,
  requireUpdater,
} = {}) {
  if (typeof appPath !== "string" || appPath.trim() === "") fail("A packaged app path is required.");
  if (process.platform !== "darwin") fail("The packaged macOS smoke test requires macOS.");
  const limit = boundedTimeout(timeoutMs);
  const updaterRequired = requireUpdater
    ?? publicChannelCompiledIn(await readFile(RELEASE_CHANNEL_FILE, "utf8"));
  const profile = await realpath(await mkdtemp(path.join("/private/tmp", PROFILE_PREFIX)));
  await chmod(profile, 0o700);
  const {
    dataRoot, fakeHome, defaultDirectory, fakeClaude, fakeAntigravity, fakeGrok, fakeQwen,
    fakeQwenMlxServer,
  } = packagedSmokePaths(profile);
  await mkdir(dataRoot, { recursive: true, mode: 0o700 });
  await mkdir(defaultDirectory, { recursive: true, mode: 0o700 });
  await mkdir(path.dirname(fakeAntigravity), { recursive: true, mode: 0o700 });
  await chmod(fakeHome, 0o700);
  await writeFile(fakeClaude, [
    "#!/bin/sh",
    // This runs below Electron -> sandbox-exec -> Rust host -> provider, so
    // success requires the restriction to survive the complete spawn chain.
    'if /usr/bin/touch "${INTERFACE_PACKAGED_SMOKE_PROFILE}-provider-write-probe" 2>/dev/null; then',
    "  printf 'packaged provider escaped its filesystem sandbox\\n' >&2; exit 70",
    "fi",
    "redraw_target=0",
    "last_line=''",
    "frame_sequence=0",
    "emit_frame() {",
    "  dimensions=$(/bin/stty size 2>/dev/null) || dimensions='24 80'",
    "  frame_rows=${dimensions%% *}",
    "  frame_columns=${dimensions##* }",
    "  frame_sequence=$((frame_sequence + 1))",
    "  printf '\\033[?1049h\\033[2J\\033[Hfake-claude-frame:%sx%s:%s\\n' \"$frame_rows\" \"$frame_columns\" \"$frame_sequence\"",
    "  if [ -n \"$last_line\" ]; then printf 'fake-claude-echo:%s\\n' \"$last_line\"; fi",
    "}",
    "claim=${INTERFACE_PACKAGED_SMOKE_REDRAW_CLAIM:-./iface-smoke-redraw-claim}",
    "if [ -t 0 ] && /bin/mkdir \"$claim\" 2>/dev/null; then",
    "  redraw_target=1",
    "  trap 'emit_frame' WINCH",
    "  /bin/dd if=/dev/zero bs=65536 count=17 2>/dev/null",
    "  printf 'fake-claude-redraw-target:%s\\n' \"$$\"",
    "  emit_frame",
    "fi",
    "printf 'fake-claude-ready:%s\\n' \"$$\"",
    "while :; do",
    "  if IFS= read -r -t 1 line; then",
    "    if [ \"$line\" = '__IFACE_FAKE_CLAUDE_EXIT__' ]; then exit 0; fi",
    "    case \"$line\" in __IFACE_PRESERVED_PROBE_*__)",
    "      printf 'fake-claude-live:%s\\n' \"$line\"; continue ;; esac",
    "    last_line=$line",
    "    printf 'fake-claude-echo:%s\\n' \"$line\"",
    "  elif [ ! -t 0 ]; then",
    "    exit 0",
    "  fi",
    "done",
    "",
  ].join("\n"), { mode: 0o700 });
  await chmod(fakeClaude, 0o700);
  await writeFile(fakeAntigravity, [
    "#!/bin/sh",
    "if [ \"$#\" -eq 1 ] && [ \"$1\" = \"--version\" ]; then",
    "  printf '1.1.11\\n'",
    "  exit 0",
    "fi",
    "if [ \"$#\" -eq 6 ] && [ \"$1\" = \"--print\" ] && [ \"$2\" = \"/usage\" ] && [ \"$3\" = \"--output-format\" ] && [ \"$4\" = \"json\" ] && [ \"$5\" = \"--print-timeout\" ] && [ \"$6\" = \"20s\" ]; then",
    "  printf '%s\\n' '{\"status\":\"SUCCESS\",\"conversation_id\":\"\",\"num_turns\":0,\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"thinking_tokens\":0,\"cache_read_tokens\":0,\"total_tokens\":0},\"command\":{\"name\":\"usage\",\"data\":{\"groups\":[{\"buckets\":[{\"id\":\"gemini-5h\",\"remaining_fraction\":0.4,\"reset_time\":\"2026-08-29T08:15:47Z\"},{\"id\":\"gemini-weekly\",\"remaining_fraction\":0.75,\"reset_time\":\"2026-09-02T19:59:25Z\"}]}]}}}'",
    "  exit 0",
    "fi",
    "exit 64",
    "",
  ].join("\n"), { mode: 0o700 });
  await chmod(fakeAntigravity, 0o700);
  await writeFile(fakeGrok, [
    `#!${process.execPath}`,
    "const readline = require('node:readline');",
    "if (process.argv.includes('--version')) {",
    "  process.stdout.write('grok-smoke 1.0.0\\n');",
    "  process.exit(0);",
    "}",
    "const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });",
    "input.on('line', line => {",
    "  let message;",
    "  try { message = JSON.parse(line); } catch { return; }",
    "  if (message.id === undefined || message.id === null) return;",
    "  let result = {};",
    "  if (message.method === 'initialize') {",
    "    result = { protocolVersion: 1, agentInfo: { version: 'grok-smoke-acp' }, agentCapabilities: {} };",
    "  } else if (message.method === 'x.ai/models/list') {",
    "    result = { models: { availableModels: [{ modelId: 'provider-grok', name: 'Provider Grok' }] } };",
    "  } else if (message.method === '_x.ai/billing') {",
    "    result = { config: {} };",
    "  }",
    "  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: message.id, result })}\\n`);",
    "});",
    "",
  ].join("\n"), { mode: 0o700 });
  await chmod(fakeGrok, 0o700);
  await copyFile(fakeGrok, fakeQwen);
  await chmod(fakeQwen, 0o700);
  await writeFile(fakeQwenMlxServer, "#!/bin/sh\nexit 64\n", { mode: 0o700 });
  await chmod(fakeQwenMlxServer, 0o700);
  for (const directory of ["process-home", "tmp", "bin"]) {
    await mkdir(path.join(profile, directory), { mode: 0o700 });
  }
  const fakes = { claude: fakeClaude, grok: fakeGrok, qwen: fakeQwen,
    antigravity: fakeAntigravity, qwenMlxServer: fakeQwenMlxServer };
  for (const [command, fake] of Object.entries(PACKAGED_FAKE_PROVIDERS)) {
    await symlink(fakes[fake], path.join(profile, "bin", command));
  }
  const sessionEnvironment = packagedSandboxEnvironment(profile);
  const shellProfile = `export PATH='${path.join(profile, "bin").replaceAll("'", "'\\''")}':/usr/bin:/bin:/usr/sbin:/sbin\n`;
  for (const home of [fakeHome, sessionEnvironment.HOME]) {
    await writeFile(path.join(home, ".zprofile"), shellProfile, { mode: 0o600 });
    await writeFile(path.join(home, ".profile"), shellProfile, { mode: 0o600 });
  }
  const sandboxPolicy = packagedSandboxPolicy(profile);
  await writeFile(path.join(profile, "host.sb"), sandboxPolicy, { mode: 0o600 });
  // The real probes below pass their paths as argv; never test by touching live data.
  const identityProbe = spawnSync("/usr/bin/sandbox-exec", ["-p", sandboxPolicy, "/bin/ps", "-o", "pid=", "-p", String(process.pid)]);
  const allowedProbe = spawnSync("/usr/bin/sandbox-exec", ["-p", sandboxPolicy, "/usr/bin/touch", path.join(profile, "allowed")]);
  const deniedProbe = spawnSync("/usr/bin/sandbox-exec", ["-p", sandboxPolicy, "/usr/bin/touch", profile + "-outside"]);
  const networkProbe = spawnSync("/usr/bin/sandbox-exec", ["-p", sandboxPolicy, process.execPath, "-e",
    "const s=require('node:net').connect(443,'1.1.1.1');s.on('error',e=>process.exit(e.code==='EPERM'?0:1));s.on('connect',()=>process.exit(1));setTimeout(()=>process.exit(1),2000);"], { timeout: 5000 });
  if (identityProbe.status !== 0 || String(identityProbe.stdout).trim() !== String(process.pid)
      || allowedProbe.status !== 0 || deniedProbe.status === 0 || networkProbe.status !== 0) {
    fail("The packaged OS sandbox failed its isolation probes.");
  }
  const expectedDefaultDirectory = await realpath(defaultDirectory);
  await assertReleaseHostSupportsPrivateSmokeRoot(appPath);
  const port = await reservePort();
  const launch = packagedSmokeLaunch(appPath, profile, port);
  const child = spawnImpl(launch.executable, launch.args, launch.options);
  let diagnostic = "";
  for (const stream of [child.stdout, child.stderr]) stream?.on("data", chunk => { diagnostic = (diagnostic + String(chunk)).slice(-16000); });
  try {
    await deadline(new Promise((resolve, reject) => {
      child.once("spawn", resolve);
      child.once("error", reject);
    }), limit, "Packaged app process launch");
    const target = await waitForTarget(port, child, limit, fetchImpl);
    const result = await inspectRenderer(target, limit, WebSocketImpl,
      { requireUpdater: updaterRequired });
    if (result.defaultDirectory !== expectedDefaultDirectory) {
      fail("The packaged Rust host did not derive its default directory from the private smoke home.");
    }
    return result;
  } catch (error) {
    console.error(diagnostic);
    throw error;
  } finally {
    await stopChild(child);
    await rm(profile, { recursive: true, force: true });
  }
}

export function parseSmokeCliArgs(argv) {
  if (argv.length !== 1 || typeof argv[0] !== "string" || argv[0].trim() === "") {
    fail("Usage: node scripts/smoke-packaged.mjs <Interface.app>");
  }
  return { appPath: path.resolve(argv[0]) };
}

async function main(argv) {
  const { appPath } = parseSmokeCliArgs(argv);
  const result = await smokePackagedApp({ appPath });
  console.log(`Packaged renderer and Rust host smoke passed (updater: ${
    String(result.settledUpdatePhase ?? result.updatePhase)}).`);
}

const invoked = process.argv[1] !== undefined
  && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;
if (invoked) {
  main(process.argv.slice(2)).catch(error => {
    console.error(`smoke-packaged: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  });
}
