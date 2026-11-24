import { readFileSync } from "node:fs";
import { terminalCardPreview } from "../terminal-card-preview";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  attachTerminalsBalanced, closeTerminalTab,
  EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, setTerminalWorkspaceArrangement,
  splitTerminalPane, toggleTerminalPaneMaximized,
} from "../terminal-workspace";
import {
  moveTerminalMenuIndex, terminalMenuTypeaheadIndex, TerminalProfileIcon,
} from "./TerminalMenu";
import {
  terminalPaneSplitOrientation, terminalStatusClass, terminalStatusText,
  terminalTabDropIndex, TerminalPane, TerminalPaneMoreMenu,
  type TerminalPaneTerminal,
} from "./TerminalPane";
import { TerminalPaneNewMenu } from "./TerminalPaneNewMenu";
import { TerminalViewPool } from "./TerminalViewPool";
import {
  sendTerminalViewCommand, TERMINAL_VIEW_COMMAND_EVENT, TERMINAL_VIEW_PARKING_SELECTOR,
  terminalViewHasSelection,
} from "../terminal-view-attachment";
import { DropIndicator } from "./DropIndicator";
import {
  TAIL_SCAN_ROWS, TERMINAL_LINE_HEIGHT, TERMINAL_SCREEN_READER_MODE, TerminalView,
  terminalHostCanFit, terminalTailLines, type TerminalTailBuffer,
} from "./TerminalView";
import {
  TerminalLayoutItems, terminalPaneMembers, TerminalSplitWorkspace, TerminalWorkspace,
  type TerminalMenuModel, type TerminalWorkspaceCallbacks,
} from "./TerminalWorkspace";

const terminalCss = readFileSync(new URL("../terminal.css", import.meta.url), "utf8");
const codiconCss = readFileSync(new URL("../codicons.css", import.meta.url), "utf8");
const themeCss = readFileSync(new URL("../theme.css", import.meta.url), "utf8");
const terminalViewSource = readFileSync(new URL("./TerminalView.tsx", import.meta.url), "utf8");
const poolSource = readFileSync(new URL("./TerminalViewPool.tsx", import.meta.url), "utf8");
const terminalWorkspaceSource = readFileSync(
  new URL("./TerminalWorkspace.tsx", import.meta.url), "utf8");
const terminalCenterSource = readFileSync(
  new URL("./IntegratedTerminalCenter.tsx", import.meta.url), "utf8");

const terminals: TerminalPaneTerminal[] = [
  { id: "terminal-a", title: "API server", cwd: "/work/interface", status: "running",
    active: true },
  { id: "terminal-b", title: "Tests", cwd: "/work/interface", status: "exited",
    exitCode: 0 },
];

describe("terminal popovers", () => {
  it("wraps keyboard movement and typeahead without selecting disabled structure", () => {
    expect(moveTerminalMenuIndex(3, 2, "next")).toBe(0);
    expect(moveTerminalMenuIndex(3, 0, "previous")).toBe(2);
    expect(moveTerminalMenuIndex(3, 1, "first")).toBe(0);
    expect(moveTerminalMenuIndex(3, 1, "last")).toBe(2);
    expect(terminalMenuTypeaheadIndex(["New terminal", "API server", "Tests"], 0, "t"))
      .toBe(2);
    expect(terminalMenuTypeaheadIndex(["New terminal"], 0, "missing")).toBe(-1);
  });

  it("keeps no rule for the retired terminal manager menu", () => {
    // The toolbar's terminals list left in 4f3853d; its selectors would only
    // read as evidence that the menu still exists.
    for (const gone of ["terminal-menu", "terminal-menu-trigger", "terminal-menu-trigger-label",
      "terminal-menu-count", "terminal-menu-new", "terminal-menu-primary",
      "terminal-menu-heading", "terminal-menu-row", "terminal-menu-copy", "terminal-menu-title",
      "terminal-menu-cwd", "terminal-menu-meta", "terminal-menu-actions", "terminal-menu-action",
      "terminal-menu-summary"]) {
      const selector = new RegExp(`\\.${gone}(?![-\\w])`, "u");
      expect(terminalCss, gone).not.toMatch(selector);
      expect(themeCss, gone).not.toMatch(selector);
    }
  });

  it("draws a provider mark for a split that inherited its source session", () => {
    // Profile-less splits keep the source's session association, which is
    // enough to show its provider without relaunching it.
    const icon = renderToStaticMarkup(createElement(TerminalProfileIcon, {
      sessionKey: "acp:grok#session-a",
    }));
    expect(icon).toContain('data-provider-logo="grok"');
    expect(icon).not.toContain("codicon-terminal");
    expect(renderToStaticMarkup(createElement(TerminalProfileIcon, {
      profileId: "shell", sessionKey: "acp:grok#session-a",
    }))).toContain("codicon-terminal");
  });

  it("offers Continue as thread on an agent pane's ··· menu, and says why when it cannot (review C1)", () => {
    let continued = 0;
    const offered = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      onContinueAsThread: () => { continued++; }, continueRefusal: null,
    }));
    expect(offered).toContain("<span>Continue as thread</span>");
    expect(offered).toContain("Only one side drives a session at a time.");
    expect([...offered.matchAll(/role="menuitem"[^>]*?tabindex="(-?\d)"/gu)].map(([, value]) => value))
      .toEqual(["0", "-1", "-1"]);
    const refused = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      onContinueAsThread: () => { continued++; },
      continueRefusal: "Wait for the agent to finish, or stop it, first.",
    }));
    expect(refused).toMatch(/disabled=""[^>]*title="Wait for the agent to finish, or stop it, first\."/u);
    expect(renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
    }))).not.toContain("Continue as thread");
    expect(continued).toBe(0);
  });

  it("gives each menu one tab stop, so Tab leaves it and the arrows move inside", () => {
    const tabStops = (markup: string): string[] =>
      [...markup.matchAll(/role="menuitem"[^>]*?tabindex="(-?\d)"/gu)].map(([, value]) => value);
    const more = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
    }));
    expect(tabStops(more)).toEqual(["0", "-1"]);
    // The launcher rows are menu items too, in the pane's `+` and again on the
    // ··· menu's Pass off page, where the Back button takes the first slot.
    const launchers = [
      { id: "shell" as const, label: "Shell", available: true },
      { id: "claude" as const, engine: "claude" as const, label: "Claude", available: true },
      { id: "codex" as const, label: "Codex", available: false,
        unavailableReason: "Codex is not installed" },
    ];
    const plus = renderToStaticMarkup(createElement(TerminalPaneNewMenu, {
      open: true, profiles: launchers,
      onOpenChange: () => undefined, onChoose: () => undefined,
    }));
    expect(tabStops(plus)).toEqual(["0", "-1", "-1"]);
    const passOff = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles: launchers, passOffEnabled: true, onPassOff: () => undefined,
      initialPage: "pass-off" as const,
    }));
    // Back, then the two launchers that survive the shell filter.
    expect(tabStops(passOff)).toEqual(["0", "-1", "-1"]);
  });
});

describe("terminal pane and workspace", () => {
  let state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "terminal-a");
  state = splitTerminalPane(state, {
    paneId: "pane-a", newPaneId: "pane-b", splitId: "split-a",
    terminalId: "terminal-b", orientation: "horizontal",
  });

  const callbacks: TerminalWorkspaceCallbacks = {
    onFocusTerminal: () => undefined,
    onDetachTerminal: () => undefined,
    onMoveTerminal: () => undefined,
    onFocusPane: () => undefined,
    onRequestSplit: () => undefined,
    onRequestPassOff: () => undefined,
    onClearTerminal: () => undefined,
    onSplitRatioChange: () => undefined,
    onToggleMaximize: () => undefined,
    onTidy: () => undefined,
  };

  // Review A3 (2026-10-14): an ended agent pane offers the host's restart.
  it("offers Start again on an ended agent pane, above xterm's own layers", () => {
    const pane = { id: "pane-x", tabs: [{ terminalId: "x" }], activeTerminalId: "x" };
    const ended = { id: "x", title: "Claude Code", cwd: "/tmp", status: "exited" as const,
      profileId: "claude", exitCode: 1, restartOffer: "Exited with code 1" };
    const render = (terminal: typeof ended | (Omit<typeof ended, "restartOffer"> & { restartOffer: null }),
        onRestartTerminal?: (id: string) => void) => renderToStaticMarkup(createElement(TerminalPane, {
      pane: pane as never, terminals: new Map([[terminal.id, terminal as never]]),
      focused: true, maximized: false, onFocus: () => undefined, onSelectTab: () => undefined,
      onCloseTab: () => undefined, onMoveTerminal: () => undefined, onRequestSplit: () => undefined,
      onToggleMaximize: () => undefined, onRestartTerminal,
      renderTerminal: () => createElement("span"),
    }));
    const html = render(ended, () => undefined);
    expect(html).toContain('class="terminal-restart-offer" role="status"');
    expect(html).toContain(">Exited with code 1</span>");
    expect(html).toContain(">Start again</button>");
    expect(render(ended)).not.toContain("terminal-restart-offer");
    expect(render({ ...ended, restartOffer: null }, () => undefined)).not.toContain("terminal-restart-offer");
    const css = readFileSync(new URL("../terminal.css", import.meta.url), "utf8");
    const rule = css.slice(css.indexOf(".terminal-restart-offer {"));
    expect(rule.slice(0, rule.indexOf("}"))).toContain("z-index: 5;");
  });

  it("keeps tabs mounted in explicit tabpanels and labels close as ending the terminal", () => {
    const pane = state.panes[0];
    const html = renderToStaticMarkup(createElement(TerminalPane, {
      pane,
      terminals: new Map(terminals.map(terminal => [terminal.id, terminal])),
      focused: true,
      maximized: false,
      onFocus: () => undefined,
      onSelectTab: () => undefined,
      onCloseTab: () => undefined,
      onMoveTerminal: () => undefined,
      onRequestSplit: () => undefined,
      onToggleMaximize: () => undefined,
      renderTerminal: ({ terminalId }: { terminalId: string }) =>
        createElement("span", { "data-rendered-terminal": terminalId }),
    }));
    expect(html).toContain('role="tablist"');
    expect(html).toContain('role="tab"');
    expect(html).toContain('draggable="true"');
    expect(html).toContain('data-terminal-drop-zone="pane"');
    expect(html).toContain('data-terminal-drop-index="0"');
    expect(html).toContain('aria-description="Drag onto another pane to swap positions, or within this pane to reorder tabs"');
    expect(html).toContain('role="tabpanel"');
    // Header order: ··· , maximize, `+`, then a × that ends the active
    // terminal. The two × no longer share one label and one tooltip: the
    // per-tab one names its terminal, the header one says it means the active
    // one, and both say what closing does instead of leaving it to a comment.
    const tabClose = 'aria-label="Close API server (ends it)"';
    const headerClose = 'aria-label="Close active terminal (ends it)"';
    expect(html.split(tabClose)).toHaveLength(2);
    expect(html.split(headerClose)).toHaveLength(2);
    expect(html).toContain('aria-label="More pane actions"');
    expect(html).toContain('aria-label="New terminal in this pane"');
    expect(html.indexOf('aria-label="More pane actions"'))
      .toBeLessThan(html.indexOf('aria-label="Maximize terminal pane"'));
    expect(html.indexOf('aria-label="Maximize terminal pane"'))
      .toBeLessThan(html.indexOf('aria-label="New terminal in this pane"'));
    expect(html.indexOf('aria-label="New terminal in this pane"'))
      .toBeLessThan(html.indexOf(headerClose));
    // The four header glyphs sit at the compact 12px tier, thickened by a
    // 0.75px text stroke (owner restored this look on 2026-09-26 over the
    // 16px icon review), and maximize/restore are the diagonal-arrow pair
    // drawn in codicons.css rather than codicon's own corner brackets, which
    // the font has no arrow version of.
    for (const glyph of ["ellipsis", "expand-diagonal", "add", "close"]) {
      expect(html).toContain(`class="codicon codicon-sm codicon-${glyph}"`);
    }
    expect(terminalCss).toMatch(/\.terminal-pane-actions \.codicon \{ -webkit-text-stroke: 0\.75px currentColor; \}/u);
    // The masks match that stroked weight.
    expect(codiconCss).toMatch(/\.codicon-expand-diagonal::before \{\s*mask-image:[^;]*stroke-width='2\.25'/u);
    expect(codiconCss).toMatch(/\.codicon-collapse-diagonal::before \{\s*mask-image:[^;]*stroke-width='2\.25'/u);
    expect(html).not.toContain("codicon-screen-full");
    expect(codiconCss).toMatch(/\.codicon-expand-diagonal::before[\s\S]*?mask:/u);
    expect(codiconCss).toMatch(/\.codicon-collapse-diagonal::before[\s\S]*?mask:/u);
    // The splits moved into the ··· menu rather than out of the pane.
    const more = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
    }));
    expect(more).toContain('class="terminal-menu-panel is-compact is-floating"');
    expect(more).toContain('aria-label="Pane actions"');
    expect(more).toContain('aria-label="Split terminal right"');
    expect(more).toContain('aria-label="Split terminal down"');
    expect(more).toMatch(/codicon-split-horizontal"[^>]*><\/span><span>Split right/u);
    expect(more).toMatch(/codicon-split-vertical"[^>]*><\/span><span>Split down/u);
    expect(more).toContain('aria-keyshortcuts="Meta+D"');
    expect(more).toContain('aria-keyshortcuts="Meta+Shift+D"');
    expect(more).toContain("Split right (⌘D)");
    expect(more).toContain("Split down (⌘⇧D)");
    expect(html).toContain('data-rendered-terminal="terminal-a"');
  });

  it("inherits a provider mark from the session without relabeling an explicit shell", () => {
    const renderTab = (profileId?: string): string => renderToStaticMarkup(createElement(
      TerminalPane,
      {
        pane: state.panes[0],
        terminals: new Map([[terminals[0].id, {
          ...terminals[0], profileId, sessionKey: "codex#session-a",
        }]]),
        focused: true,
        maximized: false,
        onFocus: () => undefined,
        onSelectTab: () => undefined,
        onCloseTab: () => undefined,
        onMoveTerminal: () => undefined,
        onRequestSplit: () => undefined,
        onToggleMaximize: () => undefined,
        renderTerminal: () => null,
      },
    ));
    const providerTab = /class="terminal-tab"[\s\S]*?<\/button>/u
      .exec(renderTab())?.[0] ?? "";
    expect(providerTab).toContain('data-provider-logo="codex"');
    expect(providerTab).not.toContain("codicon-terminal");
    const shellTab = /class="terminal-tab"[\s\S]*?<\/button>/u
      .exec(renderTab("shell"))?.[0] ?? "";
    expect(shellTab).toContain("codicon-terminal");
    expect(shellTab).not.toContain("data-provider-logo");
  });

  it("speaks the app's status vocabulary: a still dot, amber when it needs you", () => {
    const probes: Partial<TerminalPaneTerminal>[] = [
      { status: "starting" }, { activity: "working" }, {}, { activity: "waiting" },
      { status: "exited" }, { status: "failed", activity: "waiting" },
    ];
    const dot = (terminal: Partial<TerminalPaneTerminal>): string =>
      terminalStatusClass({ status: "running", ...terminal } as TerminalPaneTerminal,
        "terminal-tab-status");
    expect(dot({ status: "starting" })).toBe("terminal-tab-status is-starting");
    expect(dot({ activity: "working" })).toBe("terminal-tab-status is-working");
    expect(dot({})).toBe("terminal-tab-status is-running");
    expect(dot({ activity: "waiting" })).toBe("terminal-tab-status is-waiting");
    expect(dot({ status: "exited" })).toBe("terminal-tab-status is-exited");
    // A failure outranks a request for input.
    expect(dot({ status: "failed", activity: "waiting" })).toBe("terminal-tab-status is-failed");
    // No terminal dot animates any more; the ticker is what says "busy".
    for (const probe of probes) expect(dot(probe)).not.toContain("dot-pulse");
    expect(terminalStatusText({ status: "running", activity: "working" })).toBe("Working");
    expect(terminalStatusText({ status: "running", activity: "waiting" }))
      .toBe("Needs attention");
    expect(terminalStatusText({ status: "running" })).toBe("Idle");
    // Every state the class names has a colour. A live shell with nothing
    // happening in it is grey: green means busy, not merely alive.
    for (const state of ["starting", "working", "running", "waiting", "exited", "failed"]) {
      expect(terminalCss).toContain(`.terminal-card-status.is-${state} {`);
    }
    // Porcelain status lights (owner, 2026-09-26): working and waiting are the
    // dot plus a faint halo in the same hue, spent once through currentColor.
    expect(terminalCss).toMatch(/\.terminal-card-status\.is-waiting \{[^}]*color: var\(--status-waiting\);[^}]*background: currentColor;/u);
    // Alive-but-idle draws nothing (owner, 2026-09-12); only busy, waiting and failed paint.
    expect(terminalCss).toContain(".terminal-card-status.is-running { background: transparent; }");
    expect(terminalCss).toMatch(/\.terminal-card-status\.is-working \{[^}]*color: var\(--status-working\);[^}]*background: currentColor;/u);
  });

  it("paints a finished, unseen agent turn blue until something outranks it", () => {
    const dot = (terminal: Partial<TerminalPaneTerminal>): string =>
      terminalStatusClass({ status: "running", ...terminal } as TerminalPaneTerminal,
        "terminal-card-status");
    expect(dot({ done: true })).toBe("terminal-card-status is-done");
    // Asking for input, new work, exiting and failing all outrank done.
    expect(dot({ done: true, activity: "waiting" })).toBe("terminal-card-status is-waiting");
    expect(dot({ done: true, activity: "working" })).toBe("terminal-card-status is-working");
    expect(dot({ done: true, status: "exited" })).toBe("terminal-card-status is-exited");
    expect(dot({ done: true, status: "failed" })).toBe("terminal-card-status is-failed");
    expect(dot({ done: true, status: "starting" })).toBe("terminal-card-status is-starting");
    // Colour never carries it alone.
    expect(terminalStatusText({ status: "running", done: true })).toBe("Done");
    expect(terminalCss).toContain(".terminal-card-status.is-done { background: var(--status-done); }");
    expect(terminalCss).toContain(".terminal-tab-status.is-done,");
  });

  it("tickers a busy tab and leaves a waiting or finished one still", () => {
    const tab = (terminal: TerminalPaneTerminal): string =>
      renderToStaticMarkup(createElement(TerminalPane, {
        pane: state.panes[0],
        terminals: new Map([[terminal.id, terminal]]),
        focused: true,
        maximized: false,
        onFocus: () => undefined,
        onSelectTab: () => undefined,
        onCloseTab: () => undefined,
        onMoveTerminal: () => undefined,
        onRequestSplit: () => undefined,
        onToggleMaximize: () => undefined,
        renderTerminal: () => null,
      }));
    const working: TerminalPaneTerminal = { ...terminals[0], activity: "working" };
    const waiting: TerminalPaneTerminal = { ...terminals[0], activity: "waiting" };
    const finished: TerminalPaneTerminal = {
      ...terminals[0], status: "exited", activity: "working",
    };
    // No ticker beside the title: the dot's hue already says "working", and
    // the CLI in the pane draws its own spinner.
    expect(tab(working)).not.toContain("terminal-tab-ticker");
    expect(terminalCss).not.toContain("terminal-tab-ticker");
    expect(terminalCss).not.toContain("@keyframes terminal-tick");
    expect(terminalStatusClass(working, "terminal-tab-status")).toBe("terminal-tab-status is-working");
    expect(terminalStatusClass(waiting, "terminal-tab-status")).toBe("terminal-tab-status is-waiting");
    expect(terminalStatusClass(finished, "terminal-tab-status")).toBe("terminal-tab-status is-exited");
    // One tab needs no × of its own: the header's already closes it.
    expect(terminalCss).toContain(
      ".terminal-tab-wrap:only-child .terminal-tab-close { display: none; }");
  });

  it("announces only the two states that need the human, and not on arrival", () => {
    const html = renderToStaticMarkup(createElement(TerminalPane, {
      pane: state.panes[0],
      terminals: new Map([[terminals[0].id,
        { ...terminals[0], activity: "waiting" as const }]]),
      focused: true,
      maximized: false,
      onFocus: () => undefined,
      onSelectTab: () => undefined,
      onCloseTab: () => undefined,
      onMoveTerminal: () => undefined,
      onRequestSplit: () => undefined,
      onToggleMaximize: () => undefined,
      renderTerminal: () => null,
    }));
    // The region exists and is visually hidden; a pane that ARRIVES holding a
    // waiting terminal reports nothing, because nothing changed while anyone
    // was there to hear it.
    expect(html).toContain('<span class="terminal-sr-only" role="status"></span>');
    // The source states the rest of the contract: idle/working flips on every
    // burst of output, so only waiting and failed are spoken, and the line is
    // emptied afterwards so a sentence about a state the terminal has already
    // left is not left sitting in the accessibility tree.
    const source = readFileSync(new URL("./TerminalPane.tsx", import.meta.url), "utf8");
    expect(source).toContain('if (kind !== "waiting" && kind !== "failed") continue;');
    expect(source).toContain(
      "spoken.push(`${terminal?.title.trim() || \"Terminal\"}: ${terminalStatusText(terminal)}`);");
    expect(source).toContain('if (previous === undefined || previous === kind) continue;');
    expect(source).toContain('setAnnouncement(""); }, 1000);');
    // The words are the vocabulary already on screen, never a second one.
    expect(terminalStatusText({ status: "running", activity: "waiting" }))
      .toBe("Needs attention");
    expect(terminalStatusText({ status: "failed" })).toBe("Failed");
  });

  it("splits a wide pane side by side and a tall one top to bottom", () => {
    expect(terminalPaneSplitOrientation(800, 400)).toBe("horizontal");
    expect(terminalPaneSplitOrientation(400, 800)).toBe("vertical");
    expect(terminalPaneSplitOrientation(500, 500)).toBe("horizontal");
    // An unmeasured pane still splits like "Split right" rather than not at all.
    expect(terminalPaneSplitOrientation(0, 0)).toBe("horizontal");
    expect(terminalPaneSplitOrientation(Number.NaN, 400)).toBe("horizontal");
  });

  it("opens a New pane menu from the + with tools, launchers and only real chords", () => {
    const html = renderToStaticMarkup(createElement(TerminalPaneNewMenu, {
      open: true, cwd: "/fixture/project",
      profiles: [
        { id: "shell" as const, label: "Shell", available: true },
        { id: "claude" as const, engine: "claude" as const, label: "Claude", available: true },
      ],
      onOpenChange: () => undefined, onChoose: () => undefined, onOpenTool: () => undefined,
      onSplitDown: () => undefined, onNewTab: () => undefined, onNewSession: () => undefined,
    }));
    expect(html).toContain('class="terminal-pane-menu-heading" role="presentation"><strong>New pane</strong><span title="/fixture/project">');
    const labels = [...html.matchAll(/role="menuitem"[\s\S]*?<span>([^<]+)<\/span>/gu)].map(([, label]) => label);
    expect(labels).toEqual(["Terminal", "Files", "Browser", "iOS Simulator", "Claude",
      "Split down", "New tab", "New session…"]);
    // One glyph, one meaning: a globe browses, a phone simulates, and the split
    // is an outline pane divided once rather than the filled sidebar glyph.
    const glyphs = [...html.matchAll(/role="menuitem"[\s\S]*?class="codicon codicon-([a-z-]+)"[\s\S]*?<span>([^<]+)<\/span>/gu)]
      .map(([, glyph, label]) => `${label}:${glyph}`);
    expect(glyphs).toEqual(expect.arrayContaining(["Browser:globe", "iOS Simulator:device-mobile",
      "Split down:split-vertical"]));
    expect(html).not.toContain("layout-sidebar");
    expect(terminalCss).not.toContain(".codicon-device-mobile::before");
    expect(html.match(/role="separator"/gu)).toHaveLength(2);
    // Only chords the terminal centre binds are printed: ⌘D and ⌘⇧D.
    expect([...html.matchAll(/class="terminal-menu-shortcut" aria-hidden="true">([^<]+)</gu)]
      .map(([, chord]) => chord)).toEqual(["⌘D", "⌘⇧D"]);
    expect(html).toContain('aria-keyshortcuts="Meta+D"');
    expect(html).toContain('aria-keyshortcuts="Meta+Shift+D"');
    expect(terminalCss).toMatch(/\.terminal-menu-shortcut \{[^}]*margin-left:auto;/u);
    // The ··· no longer repeats any of it.
    const more = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
    }));
    expect(more).not.toContain("New pane");
    expect(more).not.toContain("New tab");
  });

  it("offers every launcher, disabled ones included, from the pane's + menu", () => {
    const html = renderToStaticMarkup(createElement(TerminalPaneNewMenu, {
      open: true,
      profiles: [
        { id: "shell" as const, label: "Shell", available: true },
        { id: "claude" as const, engine: "claude" as const, label: "Claude", available: true },
        { id: "codex" as const, label: "Codex", available: false,
          unavailableReason: "Codex is not installed" },
        { id: "gemini" as const, engine: "acp:gemini" as const,
          label: "Antigravity", available: true },
      ],
      onOpenChange: () => undefined,
      onChoose: () => undefined,
    }));
    expect(html).toContain('aria-label="New terminal in this pane"');
    expect(html).toContain('aria-label="Open in a new pane"');
    expect(html).toContain('class="terminal-menu-panel is-compact is-floating terminal-pane-new-menu"');
    expect(html.match(/role="menuitem"/g)).toHaveLength(4);
    expect(html).toContain('title="Codex is not installed"');
    expect(html).toContain('data-terminal-launch-profile="gemini"');
    expect(html).toContain("Antigravity");
    expect(html).toContain('data-provider-logo="antigravity"');
    expect(html).not.toContain(">Gemini<");
    expect(renderToStaticMarkup(createElement(TerminalProfileIcon, { profileId: "gemini" })))
      .toContain('data-provider-logo="antigravity"');
    expect(html).toContain("disabled");
    expect(terminalCss).toContain(".terminal-menu-panel.is-compact { min-width: 200px; }");
    // Owner reference 2026-09-14: a neutral fill, not the accent ring, marks the focused row.
    expect(terminalCss).toContain('.terminal-menu-panel.is-compact [role^="menuitem"]:focus-visible { outline: none; }');
    expect(terminalCss).toContain(".terminal-menu-launch:disabled");
  });

  it("offers Pass off to… behind the ⋯ menu and lists launchers on its page", () => {
    const profiles = [
      { id: "claude" as const, engine: "claude" as const, label: "Claude Code", available: true },
      { id: "codex" as const, label: "Codex", available: false,
        unavailableReason: "Codex is not installed" },
      { id: "grok" as const, engine: "acp:grok" as const, label: "Grok", available: true },
    ];
    const actions = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, passOffEnabled: true, onPassOff: () => undefined,
    }));
    expect(actions).toContain("Pass off to…");
    const page = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, passOffEnabled: true, onPassOff: () => undefined,
      initialPage: "pass-off" as const,
    }));
    expect(page).toContain("Claude Code");
    expect(page).toContain('data-terminal-launch-profile="grok"');
    expect(page).toContain('title="Codex is not installed"');
    expect(page).not.toContain("Split right");
    // A plain shell cannot pick up a transcript, so it is not a destination
    // here even though the + menu offers it.
    const withShell = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles: [{ id: "shell" as const, label: "Shell", available: true }, ...profiles],
      passOffEnabled: true, onPassOff: () => undefined,
      initialPage: "pass-off" as const,
    }));
    expect(withShell).toContain("Claude Code");
    expect(withShell).not.toContain("<span>Shell</span>");
    // The entry is dead while the source has nothing running to hand over, and
    // names the missing condition rather than failing silently on click.
    const item = (html: string): string =>
      /aria-label="Pass off to another agent"[\s\S]*?<\/button>/u.exec(html)?.[0] ?? "";
    expect(item(actions)).not.toContain("disabled");
    const idle = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, onPassOff: () => undefined,
    }));
    expect(item(idle)).toContain("disabled");
    expect(item(idle)).toContain('title="Pass off needs a running source terminal."');
  });

  it("leaves Muse Code out of Pass off because its TUI takes no initial prompt", () => {
    const profiles = [
      { id: "claude" as const, engine: "claude" as const, label: "Claude Code", available: true },
      { id: "muse" as const, engine: "acp:muse" as const, label: "Muse Code", available: true },
    ];
    const plus = renderToStaticMarkup(createElement(TerminalPaneNewMenu, {
      open: true, profiles, onOpenChange: () => undefined, onChoose: () => undefined,
    }));
    expect(plus).toContain('data-terminal-launch-profile="muse"');
    const page = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, passOffEnabled: true, onPassOff: () => undefined,
      initialPage: "pass-off" as const,
    }));
    expect(page).toContain('data-terminal-launch-profile="claude"');
    expect(page).not.toContain('data-terminal-launch-profile="muse"');
  });

  it("offers Kimi Code in Pass off because the host types the brief into its TUI", () => {
    const profiles = [
      { id: "claude" as const, engine: "claude" as const, label: "Claude Code", available: true },
      { id: "kimi" as const, engine: "acp:kimi" as const, label: "Kimi Code", available: true },
    ];
    const plus = renderToStaticMarkup(createElement(TerminalPaneNewMenu, {
      open: true, profiles, onOpenChange: () => undefined, onChoose: () => undefined,
    }));
    expect(plus).toContain('data-terminal-launch-profile="kimi"');
    const page = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, passOffEnabled: true, onPassOff: () => undefined,
      initialPage: "pass-off" as const,
    }));
    expect(page).toContain('data-terminal-launch-profile="claude"');
    expect(page).toContain('data-terminal-launch-profile="kimi"');
  });

  it("computes deterministic before/after insertion points for tab drops", () => {
    expect(terminalTabDropIndex(2, 109, 100, 20)).toBe(2);
    expect(terminalTabDropIndex(2, 110, 100, 20)).toBe(3);
    expect(terminalTabDropIndex(2, Number.NaN, 100, 20)).toBe(2);
  });

  /** The ··· menu's terminal layout section, as the window menu draws it. */
  const layoutMenu = (over: Partial<TerminalMenuModel> & {
    onOpenTasks?: (() => void) | undefined;
    onOpenSavedSessions?: (() => void) | undefined;
  } = {}): string => {
    const { onOpenTasks, onOpenSavedSessions, ...model } = over;
    return renderToStaticMarkup(createElement("div", { role: "menu" },
      createElement(TerminalLayoutItems, {
        menu: {
          cwd: "/Users/me/Desktop/interface", view: "split", equalGrid: false,
          disabled: false, canTidy: true, onFlip: () => undefined, onTidy: () => undefined,
          ...model,
        },
        choose: action => { action(); },
        onOpenTasks: "onOpenTasks" in over ? onOpenTasks : () => undefined,
        onOpenSavedSessions: "onOpenSavedSessions" in over ? onOpenSavedSessions : () => undefined,
      })));
  };
  const layoutRow = (html: string, label: string): string =>
    [...html.matchAll(/<button[\s\S]*?<\/button>/gu)].map(match => match[0])
      .find(button => button.includes(`<span>${label}</span>`)) ?? "";

  it("draws no bar of its own: its controls ride the window toolbar", () => {
    const html = renderToStaticMarkup(createElement(TerminalWorkspace, {
      state, terminals, activeCwd: "/work/interface", callbacks,
      renderTerminal: () => null,
    }));
    // The tiles start directly under the title row. With no toolbar slot the
    // controls are not drawn at all (the server renderer has no portals); in
    // the app they are portaled into App's .workspace-toolbar-terminal.
    expect(html).not.toContain("terminal-center-bar");
    expect(html).not.toContain("Tidy panes");
    expect(html.indexOf('class="terminal-center"')).toBeLessThan(html.indexOf('class="terminal-center-body"'));
    // Only the Output preview toggle is portaled into the toolbar; the layout
    // controls are published to the window's ··· menu (owner, 2026-09-26).
    expect(terminalWorkspaceSource).toContain("toolbarSlot !== undefined && toolbarSlot !== null && outputPreview !== undefined");
    expect(terminalWorkspaceSource).toContain("onMenuChange?.({");
    expect(terminalWorkspaceSource).not.toContain('aria-label="Terminal layout"');
    expect(terminalCss).not.toContain(".terminal-center-bar");
    expect(terminalCss).not.toContain(".terminal-center-cwd");
    expect(terminalWorkspaceSource).not.toContain("terminal-center-toolbar");
    expect(terminalWorkspaceSource).not.toContain("setActionsOpen");
  });

  it("keeps the reference layout rows, now inside the window's ··· menu", () => {
    const open = layoutMenu();
    // Folder heading first, the way the pane menu names where it will act.
    expect(open).toMatch(/class="terminal-pane-menu-heading"[^>]*><strong>interface<\/strong><span title="\/Users\/me\/Desktop\/interface">~\/Desktop\/interface<\/span>/u);
    const rows = [...open.matchAll(/<button[^>]*role="(menuitem(?:checkbox)?)"[^>]*>([\s\S]*?)<\/button>/gu)];
    expect(rows.map(row => row[2].replace(/<[^>]+>/gu, ""))).toEqual([
      "Tidy panes", "Flip terminals", "Tasks", "Codex accounts…", "Saved sessions…",
    ]);
    expect(rows.map(row => row[1])).toEqual([
      "menuitemcheckbox", "menuitemcheckbox", "menuitem", "menuitem", "menuitem",
    ]);
    expect(rows.map(row => /codicon-([a-z-]+)"/u.exec(row[2])?.[1])).toEqual([
      "layout-grid", "arrow-swap", "checklist", "account", "history",
    ]);
    // One divider, between the layout pair and the destinations. Usage moved
    // up to the ··· menu's own row, so it is not repeated here.
    expect(open.match(/role="separator"/g)).toHaveLength(1);
    expect(open.indexOf('role="separator"')).toBeGreaterThan(open.indexOf("Flip terminals"));
    expect(open.indexOf('role="separator"')).toBeLessThan(open.indexOf("<span>Tasks</span>"));
    expect(open).not.toContain("Usage");
    // The Live panes toggle rides here too, checked while the column is open.
    const withOverview = layoutMenu({ overview: { open: true, onToggle: () => undefined } });
    expect(layoutRow(withOverview, "Live panes")).toContain('role="menuitemcheckbox"');
    expect(layoutRow(withOverview, "Live panes")).toContain('aria-checked="true"');
    expect(layoutRow(withOverview, "Live panes")).toContain("codicon-layout-sidebar-right");
    expect(open).not.toContain("Live panes");

    // Tidy is still a two-way toggle: the automatic grid draws no splitter
    // bars, so the same row has to lead back out or the layout cannot be
    // resized again. The check mark says which way it will go.
    expect(layoutRow(open, "Tidy panes")).toContain('aria-checked="false"');
    expect(layoutRow(open, "Tidy panes")).not.toContain("codicon-check ");
    const grid = layoutMenu({ equalGrid: true });
    expect(layoutRow(grid, "Tidy panes")).toContain('aria-checked="true"');
    expect(layoutRow(grid, "Tidy panes")).toContain('class="codicon codicon-check terminal-menu-check"');
    // Tidy needs two panes.
    expect(layoutRow(layoutMenu({ canTidy: false }), "Tidy panes")).toContain('disabled=""');
    expect(layoutRow(open, "Tidy panes")).not.toContain("disabled");

    // A destination App has not wired is not offered, and nothing names a
    // folder when there is none. Codex accounts is a placeholder row until
    // account switching exists, so it is always there.
    const bare = layoutMenu({ cwd: null, onOpenTasks: undefined,
      onOpenSavedSessions: undefined });
    expect(bare).not.toContain("terminal-pane-menu-heading");
    expect(bare).not.toContain("<span>Tasks</span>");
    expect(bare).not.toContain("Saved sessions…");
    expect(bare).toContain("<span>Codex accounts…</span>");

    for (const name of ["layout-grid", "arrow-swap", "dashboard", "checklist", "account", "history", "check"]) {
      expect(codiconCss, `missing .codicon-${name}`).toContain(`.codicon-${name}::before`);
    }
    expect(terminalWorkspaceSource).toContain(
      '<div key="terminal-split" className="terminal-center-split"',
    );
    // The card grid is gone: a detached terminal is listed, with the same five
    // actions, by the manager menu.
    expect(terminalWorkspaceSource).not.toContain("TerminalOverview");
    expect(terminalCss).not.toContain(".terminal-overview-card");
  });

  it("flips every pane to a title card without unmounting the terminals", () => {
    const html = renderToStaticMarkup(createElement(TerminalWorkspace, {
      state, terminals, activeCwd: "/work/interface", callbacks, view: "cards",
      renderTerminal: ({ terminalId }: { terminalId: string }) =>
        createElement("span", { "data-terminal-body": terminalId }),
    }));
    // Both panes keep their place in the split; only the bodies are covered.
    expect(html.match(/class="terminal-pane-card"/g)).toHaveLength(2);
    expect(html).toContain('class="terminal-pane-card-title" title="API server">API server<');
    expect(html).toContain('class="terminal-pane-card-title" title="Tests">Tests<');
    // The card's name is built from the strings the card prints, folder
    // included — not a shorter override that leaves them to the sighted.
    expect(html).toContain('aria-label="API server, Idle, /work/interface"');
    expect(html).toContain('aria-label="Tests, Exited, /work/interface"');
    // Covered, never unmounted: unmounting an xterm loses its buffer.
    expect(html).toContain('data-terminal-body="terminal-a"');
    expect(html).toContain('data-terminal-body="terminal-b"');
    // The layout menu's Flip row reads as checked and leads back while the cards are up.
    expect(layoutRow(layoutMenu({ view: "cards" }), "Flip terminals")).toContain('aria-checked="true"');
    expect(layoutRow(layoutMenu(), "Flip terminals")).toContain('aria-checked="false"');
    expect(terminalCss).toContain(".terminal-pane-card {");
    expect(terminalCss).toContain(".terminal-pane-card-title {");
  });

  it("puts the folder, the state word and the last buffer lines on a card", () => {
    // Three shells in one folder are told apart by what they last printed, so
    // the card carries it. The buffer is readable precisely because the pane
    // underneath is covered rather than unmounted.
    const withPreview: TerminalPaneTerminal[] = [
      { ...terminals[0], preview: ["$ npm run check", "12 files changed"] },
      terminals[1],
    ];
    const cards = renderToStaticMarkup(createElement(TerminalWorkspace, {
      state, terminals: withPreview, activeCwd: "/work/interface", callbacks, view: "cards",
      renderTerminal: () => null,
    }));
    const cardMarkup = cards.split('class="terminal-pane-card"');
    const card = cardMarkup[1] ?? "";
    expect(card).toContain('class="terminal-pane-card-title" title="API server">API server<');
    expect(card).toContain("Idle");
    expect(card).toContain('class="terminal-card-cwd">/work/interface<');
    expect(card).toContain("$ npm run check\n12 files changed");
    // And the preview reaches the accessible name too: it is the whole reason
    // three shells in one folder are distinguishable at all.
    expect(card).toContain(
      'aria-label="API server, Idle, /work/interface, $ npm run check, 12 files changed"');
    // A terminal with no captured buffer prints no empty preview box.
    const quiet = cardMarkup[2] ?? "";
    expect(quiet).toContain('class="terminal-pane-card-title" title="Tests">Tests<');
    expect(quiet).not.toContain("terminal-card-preview");

    for (const rule of [".terminal-card-meta {", ".terminal-card-cwd {",
      ".terminal-card-preview {"]) {
      expect(terminalCss, `missing ${rule}`).toContain(rule);
    }
    // Two lines, never a wall of them.
    expect(terminalCss).toMatch(
      /\.terminal-card-preview \{[\s\S]*?-webkit-line-clamp: 2;/u);
  });

  it("takes the layout box away from a covered tab so xterm stops painting it", () => {
    // `visibility: hidden` keeps a box, and xterm's IntersectionObserver reads
    // a box as on screen: every background tab kept repainting every row.
    expect(terminalCss).toMatch(/\.terminal-tabpanel \{[\s\S]*?content-visibility: hidden;/u);
    expect(terminalCss).toMatch(
      /\.terminal-tabpanel\.is-active \{[\s\S]*?content-visibility: visible;/u);
    // Never on the flip cover: the panes behind it must keep their boxes so a
    // window resize still fits them.
    expect(terminalCss).not.toMatch(/\.terminal-pane-card \{[\s\S]*?content-visibility/u);
  });

  it.each(Array.from({ length: 6 }, (_, index) => index + 1))(
    "renders an automatic %i-pane layout as direct equal-grid cells", count => {
      const ids = Array.from({ length: count }, (_, index) => `automatic-${index + 1}`);
      const equalState = attachTerminalsBalanced(
        EMPTY_TERMINAL_WORKSPACE_STATE,
        ids,
        index => `automatic-pane-${index + 1}`,
        index => `automatic-split-${index + 1}`,
      );
      const equalTerminals: TerminalPaneTerminal[] = ids.map((id, index) => ({
        id,
        title: `Terminal ${index + 1}`,
        cwd: "/work/interface",
        status: "running",
        active: index === 0,
      }));
      const html = renderToStaticMarkup(createElement(TerminalSplitWorkspace, {
        state: equalState,
        terminals: equalTerminals,
        onFocusPane: () => undefined,
        onSelectTab: () => undefined,
        onCloseTab: () => undefined,
        onMoveTerminal: () => undefined,
        onRequestSplit: () => undefined,
        onSplitRatioChange: () => undefined,
        onToggleMaximize: () => undefined,
        renderTerminal: () => null,
      }));
      expect(html).toContain(
        `class="terminal-workspace is-equal-grid terminal-count-${count}"`,
      );
      expect(html).toContain('data-terminal-arrangement="equal-grid"');
      expect(html.match(/data-terminal-pane-id=/g)).toHaveLength(count);
      expect(html).not.toContain("data-terminal-split-id=");
    },
  );

  it.each([
    { initialCount: 4, removedIndex: 1, remainingCount: 3 },
    { initialCount: 6, removedIndex: 3, remainingCount: 5 },
  ])("reflows a $initialCount-pane grid after removal to $remainingCount filled panes",
    ({ initialCount, removedIndex, remainingCount }) => {
      const ids = Array.from({ length: initialCount }, (_, index) => `remove-${index + 1}`);
      const initial = attachTerminalsBalanced(
        EMPTY_TERMINAL_WORKSPACE_STATE,
        ids,
        index => `remove-pane-${index + 1}`,
        index => `remove-split-${index + 1}`,
      );
      const remainingIds = ids.filter((_, index) => index !== removedIndex);
      const removed = closeTerminalTab(initial, ids[removedIndex]);
      const html = renderToStaticMarkup(createElement(TerminalSplitWorkspace, {
        state: removed,
        terminals: remainingIds.map((id, index) => ({
          id,
          title: `Terminal ${index + 1}`,
          cwd: "/work/interface",
          status: "running" as const,
          active: index === 0,
        })),
        onFocusPane: () => undefined,
        onSelectTab: () => undefined,
        onCloseTab: () => undefined,
        onMoveTerminal: () => undefined,
        onRequestSplit: () => undefined,
        onSplitRatioChange: () => undefined,
        onToggleMaximize: () => undefined,
        renderTerminal: () => null,
      }));
      expect(removed.arrangement).toBe("equal-grid");
      expect(html).toContain(`terminal-count-${remainingCount}`);
      expect(html.match(/data-terminal-pane-id=/gu)).toHaveLength(remainingCount);
      expect(html).not.toContain("data-terminal-split-id=");
      let previous = -1;
      for (const id of remainingIds) {
        const at = html.indexOf(`data-terminal-id=\"${id}\"`);
        expect(at).toBeGreaterThan(previous);
        previous = at;
      }
    });

  it("draws draggable splitter bars once the automatic grid is left", () => {
    const ids = ["automatic-1", "automatic-2", "automatic-3", "automatic-4"];
    const equalState = attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE,
      ids,
      index => `automatic-pane-${index + 1}`,
      index => `automatic-split-${index + 1}`,
    );
    const html = renderToStaticMarkup(createElement(TerminalSplitWorkspace, {
      state: setTerminalWorkspaceArrangement(equalState, "custom-split"),
      terminals: ids.map((id, index) => ({
        id,
        title: `Terminal ${index + 1}`,
        cwd: "/work/interface",
        status: "running" as const,
        active: index === 0,
      })),
      onFocusPane: () => undefined,
      onSelectTab: () => undefined,
      onCloseTab: () => undefined,
      onMoveTerminal: () => undefined,
      onRequestSplit: () => undefined,
      onSplitRatioChange: () => undefined,
      onToggleMaximize: () => undefined,
      renderTerminal: () => null,
    }));
    expect(html).toContain('data-terminal-arrangement="custom-split"');
    expect(html).not.toContain("is-equal-grid");
    expect(html.match(/data-terminal-pane-id=/g)).toHaveLength(ids.length);
    // Three bars for four panes: without them the layout cannot be resized.
    expect(html.match(/class="terminal-splitter"/g)).toHaveLength(ids.length - 1);
  });

  it("maximizes a pane by hiding its siblings, never by unmounting them", () => {
    const html = renderToStaticMarkup(createElement(TerminalSplitWorkspace, {
      state: toggleTerminalPaneMaximized(state, "pane-b"),
      terminals,
      onFocusPane: () => undefined,
      onSelectTab: () => undefined,
      onCloseTab: () => undefined,
      onMoveTerminal: () => undefined,
      onRequestSplit: () => undefined,
      onSplitRatioChange: () => undefined,
      onToggleMaximize: () => undefined,
      renderTerminal: ({ terminalId }: { terminalId: string }) =>
        createElement("span", { "data-rendered-terminal": terminalId }),
    }));
    expect(html).toContain('class="terminal-workspace has-maximized"');
    // Both terminal bodies stay mounted; unmounting one disposes its xterm buffer.
    expect(html).toContain('data-rendered-terminal="terminal-a"');
    expect(html).toContain('data-rendered-terminal="terminal-b"');
    expect(html).toContain('class="terminal-pane is-focused is-maximized"');
    expect(terminalCss).toContain(
      ".terminal-workspace.has-maximized .terminal-pane-frame:not(:has(> .terminal-pane.is-maximized)),",
    );
    expect(terminalCss).toContain(
      ".terminal-workspace.has-maximized .terminal-pane.is-maximized { height: 100%; }",
    );
  });

  it("hides splitters until hover, highlights active drags, and resets on double-click", () => {
    expect(terminalWorkspaceSource).toContain("onDoubleClick");
    expect(terminalWorkspaceSource).toContain("event.detail >= 2");
    expect(terminalWorkspaceSource).toContain(
      "props.onSplitRatioChange(tree.id, DEFAULT_TERMINAL_SPLIT_RATIO)");
    // Invisible at rest, painted only on hover/focus/drag. [^}] keeps each
    // match inside its own rule block.
    expect(terminalCss).toMatch(
      /\.terminal-splitter::after \{[^}]*background: var\(--selection-border\);[^}]*opacity: 0;[^}]*\}/u,
    );
    expect(terminalCss).toMatch(
      /\.terminal-splitter:hover::after,[^}]*opacity: 1;/u,
    );
    // The reveal fades rather than snapping on.
    expect(terminalCss).toMatch(
      /\.terminal-splitter::after \{[^}]*transition: opacity var\(--motion-fast\)[^}]*\}/u,
    );
    expect(terminalCss).toContain(".terminal-splitter.is-dragging::after");
    expect(terminalCss).toContain(".terminal-splitter:active::after");
    expect(terminalWorkspaceSource).toContain("splitterDrag.start({");
    expect(terminalWorkspaceSource).toContain("splitter.setPointerCapture(pointerId)");
    expect(terminalWorkspaceSource).toContain("onLostPointerCapture");
    expect(terminalWorkspaceSource).toContain("event.button !== 0 || !event.isPrimary");
  });

  it("draws a rounded selection box around the focused terminal pane", () => {
    const pane = state.panes[0];
    const html = renderToStaticMarkup(createElement(TerminalPane, {
      pane,
      terminals: new Map(terminals.map(terminal => [terminal.id, terminal])),
      focused: true,
      maximized: false,
      onFocus: () => undefined,
      onSelectTab: () => undefined,
      onCloseTab: () => undefined,
      onMoveTerminal: () => undefined,
      onRequestSplit: () => undefined,
      onToggleMaximize: () => undefined,
      renderTerminal: () => null,
    }));
    expect(html).toContain("terminal-pane is-focused");
    // Visible stroke is a covering ::after. Pane inset is under the canvas;
    // outer spread is clipped to corners by overflow:hidden + border-radius.
    // A local stacking context keeps that overlay in the pane when a
    // position:fixed New menu would otherwise pin it to the viewport.
    expect(terminalCss).toMatch(/\.terminal-pane \{[\s\S]*?position: relative;/u);
    expect(terminalCss).toMatch(/\.terminal-pane \{[\s\S]*?isolation: isolate;/u);
    expect(terminalCss).toMatch(/\.terminal-pane \{[\s\S]*?z-index: 0;/u);
    // SETTLED LOOK, dense tiles (owner, 2026-10-01, replacing Porcelain's
    // frames): every pane is a flush TILE, its head a band over the screen; a
    // NEUTRAL grey edge, never the theme's blue-tinted --border-strong, and a
    // firmer neutral focus edge: the only colour in the grid belongs to the
    // status lights.
    expect(terminalCss).toMatch(
      /\.terminal-pane \{[\s\S]*?padding: 0;[\s\S]*?background: var\(--terminal-content-bg\);/u,
    );
    expect(themeCss).toContain("--terminal-tile-edge: #262626;");
    expect(terminalCss).toMatch(
      /\.terminal-pane\.is-focused::after \{[^}]*border-color: var\(--terminal-focus-edge\);/u,
    );
    expect(terminalCss).not.toMatch(/\.terminal-pane\.is-focused[^{]*\{[^}]*--selection-accent/u);
    // The head is a band one step above the screen, with a hairline under it...
    expect(terminalCss).toMatch(
      /\.terminal-pane-head \{[^}]*background: var\(--terminal-header-bg\);[^}]*border-bottom: var\(--border-w\) solid var\(--terminal-header-border\);/u);
    expect(themeCss).toContain("--terminal-header-bg: #151514;");
    expect(themeCss).toContain("--terminal-header-bg: #f7f7f5;");
    // ...and the tile clips its screen, so the screen draws no corners or edge.
    expect(terminalCss).toMatch(/\.terminal-pane-body \{[^}]*border-radius: var\(--tile-screen-radius\);[^}]*box-shadow: none;/u);
    // ...the pane's name stays readable in its head, one tab or many: the owner
    // reversed the reference's title-less head on 2026-09-06...
    expect(terminalCss).not.toContain(".terminal-tab-wrap:only-child .terminal-tab-title");
    expect(terminalCss).toContain(
      ".terminal-tab-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }");
    // ...nothing floats over the terminal's text: the body toolbar's actions
    // moved into the head (owner, 2026-11-15), and Find opens a row the
    // screen gives up rather than a box over the text. The scrollbar thumb
    // still appears only under the pointer...
    expect(terminalCss).not.toContain(".terminal-view-toolbar");
    expect(terminalViewSource).not.toContain("terminal-view-toolbar");
    expect(terminalCss).toContain(".terminal-view.has-search .terminal-xterm { top: var(--terminal-search-h); }");
    expect(terminalCss).toMatch(
      /\.terminal-pane:not\(:hover\) \.terminal-xterm \.xterm \.xterm-scrollable-element > \.scrollbar > \.slider:not\(\.active\) \{[^}]*background: transparent;/u,
    );
    // ...the grid's own gap, radius and head height, scoped so the agent
    // canvas keeps its own...
    expect(terminalCss).toMatch(/\.terminal-center \{[^}]*--tile-head-h: 32px;[^}]*--tile-gap: 12px;[^}]*--tile-radius: 10px;/u);
    // Text sits 12px in from the tile's edge, 10px under the head (owner, 2026-10-02).
    expect(terminalCss).toMatch(/\.terminal-xterm \{[^}]*padding: 10px 0 2px 12px;/u);
    // ...and the SELECTED tile's head band steps up one shade; nothing lifts.
    expect(terminalCss).toContain(
      ".terminal-pane.is-focused .terminal-pane-head { background: var(--terminal-header-bg-focused); }");
    expect(terminalCss).not.toContain("--terminal-frame-shadow-focused");
    expect(themeCss).toContain("--terminal-header-bg-focused: #1e1d1b;");
    expect(themeCss).toContain("--terminal-header-bg-focused: #ffffff;");
    expect(terminalCss).toMatch(
      /\.terminal-pane::after \{[^}]*pointer-events: none;/u,
    );
    expect(terminalCss).not.toMatch(
      /\.terminal-pane\.is-focused \{[\s\S]*?box-shadow: 0 0 0 var\(--border-w\) var\(--selection-accent\);/u,
    );
    expect(terminalCss).toContain(
      ".terminal-pane.is-drop-target::after {\n  border-color: var(--selection-border);\n  border-width: 2px;",
    );
  });

  it("fills automatic desktop rows after a terminal is removed", () => {
    expect(terminalCss).toContain(
      "grid-template-columns: repeat(24, minmax(0, 1fr));",
    );
    expect(terminalCss).toMatch(
      /terminal-count-3 > \.terminal-pane-frame:last-child \{\s*grid-column: 1 \/ -1;/u,
    );
    expect(terminalCss).toMatch(
      /terminal-count-5 > \.terminal-pane-frame:nth-last-child\(-n \+ 2\) \{\s*grid-column: span 12;/u,
    );
    expect(terminalCss).toContain("grid-auto-rows: minmax(180px, 1fr);");
    expect(terminalCss).toMatch(/\.terminal-workspace\.is-equal-grid\s*\{[\s\S]*?gap: var\(--tile-gap\);/u);
    expect(terminalCss).toMatch(/\.terminal-workspace\s*\{[\s\S]*?padding: 0;/u);
    expect(terminalCss).toMatch(/\.terminal-pane\s*\{[\s\S]*?border: 0;[\s\S]*?border-radius: var\(--tile-radius\);/u);
  });

  it("fills odd responsive rows and collapses every pane to one column", () => {
    expect(terminalCss).toMatch(/@media \(max-width: 1100px\)[\s\S]*?terminal-count-7[\s\S]*?last-child \{ grid-column: 1 \/ -1 !important;/u);
    expect(terminalCss).toMatch(/@media \(max-width: 640px\)[\s\S]*?terminal-pane-frame \{ grid-column: 1 \/ -1 !important;/u);
  });

  it("replaces the empty shell prompt with the reviewed agent/count launcher", () => {
    const html = renderToStaticMarkup(createElement(TerminalWorkspace, {
      state: EMPTY_TERMINAL_WORKSPACE_STATE,
      terminals: [],
      activeCwd: "/work/interface",
      callbacks,
      renderTerminal: () => null,
      launcher: {
        open: false,
        cwd: "/work/interface",
        profiles: [
          { id: "shell", label: "Shell", available: true },
          { id: "claude", engine: "claude", label: "Claude", available: true },
        ],
        profilesLoading: false,
        launching: false,
        notice: null,
        onOpen: () => undefined,
        onClose: () => undefined,
        onLaunch: () => undefined,
        onBrowse: async () => null,
        onCwdChange: () => undefined,
        onRetryProfiles: () => undefined,
      },
    }));
    expect(html).toContain('class="terminal-launcher"');
    expect(html).toContain("New workspace");
    expect(html).toContain('data-terminal-launch-profile="claude"');
    expect(html).toContain("Launch 1 Claude terminal");
    expect(html).not.toContain('data-rendered-terminal=');
  });

  /* The saved-workspace switcher is the SIDEBAR's, not this surface's: the
     center publishes it through onSwitcherChange and draws no bar of its own.
     What stays testable here is that a launch in flight locks the pane's own
     controls. */
  it("locks the pane's controls while a terminal batch is launching", () => {
    const html = renderToStaticMarkup(createElement(TerminalWorkspace, {
      state: EMPTY_TERMINAL_WORKSPACE_STATE,
      terminals: [],
      activeCwd: "/work/interface",
      callbacks,
      renderTerminal: () => null,
      launcher: {
        open: true,
        cwd: "/work/interface",
        profiles: [{ id: "shell", label: "Shell", available: true }],
        profilesLoading: false,
        launching: true,
        notice: null,
        onOpen: () => undefined,
        onClose: () => undefined,
        onLaunch: () => undefined,
        onBrowse: async () => null,
        onCwdChange: () => undefined,
        onRetryProfiles: () => undefined,
      },
    }));
    expect(html).not.toContain("terminal-workspace-bar");
    const locked = layoutMenu({ disabled: true });
    expect(layoutRow(locked, "Flip terminals")).toContain('disabled=""');
    expect(layoutRow(locked, "Tidy panes")).toContain('disabled=""');
  });
});

describe("TerminalView shell", () => {
  it("uses the owner's terminal face, MonoLisa Bold, instead of the app chrome face", () => {
    // As set in the owner's Rex terminal (2026-11-15): MonoLisa, Bold, with
    // JetBrainsMono Nerd Font behind it for the glyphs MonoLisa lacks.
    expect(themeCss).toContain(
      '--font-terminal: "MonoLisa", "JetBrainsMono Nerd Font", "Geist Mono"',
    );
    expect(themeCss).toContain("--terminal-text-weight: 700;");
    expect(terminalViewSource).toContain('cssValue(styles, "--font-terminal"');
    expect(terminalViewSource).toContain('cssValue(styles, "--terminal-text-weight", "400")');
    // Cells round to the nearest device pixel, as Ghostty's do.
    expect(terminalViewSource).toContain(
      "letterSpacing: cellSnapLetterSpacing(terminalAdvance(fontFamily, fontSize), window.devicePixelRatio)");
    expect(terminalCss).toContain("--terminal-reading-size: var(--workspace-terminal-font-size, 11.5);");
  });

  it("paints the emulator in the active theme's terminal colours", () => {
    // Palette-specific terminal tiers are reapplied to existing emulators.
    // The DOM fixture verifies switching without a remount or buffer loss.
    expect(terminalViewSource).toContain('cssValue(styles, "--terminal-content-bg"');
    expect(terminalViewSource).toMatch(
      /theme: \{[\s\S]*?\.\.\.terminalAnsiPalette\(document\.documentElement\.dataset\.theme, styles\),\s*background: cssValue\(styles, "--terminal-glass-active", "0"\) === "1" \? "#00000000" : terminalBackground,\s*foreground: cssValue\(styles, "--fg"[\s\S]*?cursor: cssValue\(styles, "--selection-accent"[\s\S]*?selectionBackground: cssValue\(styles, "--selection"/u);
  });

  it("keeps the cursor still, because blinking it is a CSS animation", () => {
    // The DOM renderer blinks with an animation that forces 120-170 style
    // recalculations a second for as long as any pane is visible.
    expect(terminalViewSource).toContain("cursorBlink: false,");
    expect(terminalViewSource).not.toContain("cursorBlink: true");
  });

  it("loads the WebGL renderer after open, and still opens a terminal without one", () => {
    const opened = terminalViewSource.indexOf("terminal.open(host);");
    const loaded = terminalViewSource.indexOf("new WebglAddon()");
    expect(opened).toBeGreaterThan(-1);
    // The addon can only attach once the terminal owns an element.
    expect(loaded).toBeGreaterThan(opened);
    expect(terminalViewSource).toContain("terminal.loadAddon(addon);");
    expect(terminalViewSource).toContain("webglUnavailable = true;");
    // Disposal on context loss is the fallback: xterm paints the same buffer
    // with the DOM renderer.
    expect(terminalViewSource).toMatch(/terminalWebglPool\.release\(webglOwner\);\s*terminal\.dispose\(\);/u);
  });

  it("hands each pane only its own terminals, and the same map while they are unchanged", () => {
    const entry = (id: string, activity: "idle" | "working" = "idle"): TerminalPaneTerminal => ({
      id, title: id, cwd: "/tmp/project", status: "running", active: true, attached: true,
      exitCode: null, activity });
    const a = entry("a");
    const b = entry("b");
    const c = entry("c");
    const pane = { tabs: [{ terminalId: "a" }, { terminalId: "b" }] };
    const first = terminalPaneMembers(undefined, pane, new Map([["a", a], ["b", b], ["c", c]]));
    expect([...first.keys()]).toEqual(["a", "b"]);
    // Another pane's terminal starting work leaves this pane's props alone.
    expect(terminalPaneMembers(first, pane,
      new Map([["a", a], ["b", b], ["c", entry("c", "working")]]))).toBe(first);
    const busy = entry("b", "working");
    const next = terminalPaneMembers(first, pane, new Map([["a", a], ["b", busy]]));
    expect(next).not.toBe(first);
    expect(next.get("b")).toBe(busy);
    // A tab whose terminal has not arrived yet reads as missing, as before.
    expect(terminalPaneMembers(next, pane, new Map([["a", a]])).has("b")).toBe(false);
  });

  it("routes parked and returning GPU renderers through the bounded cache", () => {
    // Lifecycle/resource behavior is exercised with real renderers in the
    // workspace-switch fixture; these pins cover the component's event wiring.
    expect(poolSource).toMatch(
      /parking\.current\?\.appendChild\(entry\.mount\);[\s\S]*?new Event\(TERMINAL_VIEW_DETACHMENT_EVENT\)/u);
    expect(terminalViewSource).toContain("host.addEventListener(TERMINAL_VIEW_DETACHMENT_EVENT, parked);");
    expect(terminalViewSource).toMatch(
      /webglRef\.current\?\.ensure\(\);\s*tryFit\(fitRef\.current, host, terminal\);/u);
    expect(terminalViewSource).toContain("terminalWebglPool.park(webglOwner);");
    expect(terminalViewSource).toContain("terminalWebglPool.slot(webglOwner);");
    expect(terminalViewSource).toContain("terminalWebglPool.acquire(webglOwner, disposeWebgl, webglHooks)");
    expect(terminalViewSource).toContain('context?.getExtension("WEBGL_lose_context")?.loseContext();');
    // A background tab or hidden surface parks like a detached slot, and
    // reloads in the layout effect that shows it, before that commit paints.
    expect(terminalViewSource).toMatch(
      /webglRef\.current\?\.show\(active\);\s*if \(!active\) return;[\s\S]{0,240}webglRef\.current\?\.ensure\(\);/u);
    expect(terminalViewSource).toContain("if (slotted && shown) terminalWebglPool.slot(webglOwner);");
    // Mounting into a hidden tab or the parking container takes no renderer.
    expect(terminalViewSource).not.toMatch(/^\s*loadWebgl\(\);$/mu);
  });

  it("repaints a pane parked through an appearance change on its return, never from the parking container", () => {
    // The workspace-switch fixture changes the text size with a workspace out
    // of view and checks its panes on return; this pins the wiring.
    const parking = renderToStaticMarkup(createElement(TerminalViewPool, {
      streamKeys: new Set<string>(), workingKeys: new Set<string>(), renderTerminal: () => null,
      children: null,
    }));
    expect(TERMINAL_VIEW_PARKING_SELECTOR).toBe("[data-terminal-parking]");
    expect(parking).toContain('data-terminal-parking=""');
    // Out there a pane would read the 12px fallback and the window's text
    // colour instead of its screen's.
    expect(terminalCss).toMatch(/\.terminal-center \{[^}]*--terminal-reading-size:/u);
    expect(terminalCss).toMatch(/\.terminal-pane-body \{[^}]*--fg: var\(--terminal-fg\);/u);
    expect(terminalViewSource).toMatch(
      /stale = host\.closest\(TERMINAL_VIEW_PARKING_SELECTOR\) !== null;\s*if \(stale\) return;\s*const next = interfaceOptions/u);
    expect(terminalViewSource).toContain("const returned = (): void => { if (stale) paint(); };");
    expect(terminalViewSource).toContain("host.addEventListener(TERMINAL_VIEW_ATTACHMENT_EVENT, returned);");
    expect(terminalViewSource).toContain("host.removeEventListener(TERMINAL_VIEW_ATTACHMENT_EVENT, returned);");
  });

  it("reads a card preview off the covered buffer, blanks skipped and newest last", () => {
    expect(terminalViewSource).toContain("tailLines(limit: number): string[];");
    expect(terminalViewSource).toMatch(
      /tailLines\(limit\) \{[\s\S]*?terminalTailLines\(terminal\.buffer\.active, limit\)/u);
    expect(terminalViewSource).toContain(
      "for (let y = buffer.length - 1; y >= stopAt && lines.length < limit; y -= 1)");
    expect(terminalViewSource).toContain('if (text !== "") lines.unshift(text);');
    // Read when the cards appear, not per write, and dropped on the way back.
    expect(terminalCenterSource).toContain("?.tailLines(40) ?? [];");
    expect(terminalCenterSource).toMatch(
      /if \(workspaceView === "split"\) \{\s*setPreviews\(current => \(current\.size === 0 \? current : new Map\(\)\)\);/u);
  });

  it("enables xterm's VoiceOver accessibility tree by default", () => {
    expect(TERMINAL_SCREEN_READER_MODE).toBe(true);
  });

  it("keeps adjacent block-art and box-drawing rows visually joined", () => {
    // Rows are the font's own cell since 2026-11-15 (owner, matching their Rex
    // terminal; 1.2× before that). The frames stay joined because xterm's
    // custom glyphs fill the whole row; nothing may turn them off.
    expect(TERMINAL_LINE_HEIGHT).toBe(1);
    expect(terminalViewSource).not.toContain("customGlyphs");
  });

  it("redraws the xterm bitmap after a fit, because hiding the surface discards canvases", () => {
    expect(terminalViewSource).toContain("terminal.refresh(0, terminal.rows - 1)");
    expect(terminalViewSource).toMatch(
      /function tryFit\([\s\S]*?addon\?\.fit\(\);[\s\S]*?terminal\.refresh\(0, terminal\.rows - 1\)/u,
    );
  });

  it("redraws only on the way back from an unmeasurable host, not every fit", () => {
    // A splitter drag fits every frame. Refreshing all rows each time repaints
    // the whole viewport for nothing; only a host returning from display:none
    // has a discarded canvas to restore.
    expect(terminalViewSource).toContain("const fittedHosts = new WeakSet<HTMLElement>()");
    expect(terminalViewSource).toContain("if (host !== null) fittedHosts.delete(host);");
    expect(terminalViewSource).toContain(
      "const returning = host !== null && !fittedHosts.has(host);");
    expect(terminalViewSource).toContain(
      "if (returning && terminal !== null && terminal.rows > 0)");
  });

  it("refits on layout shape, not on every splitter frame", () => {
    // A drag mints a new layout object per frame while only a ratio moves, and
    // TerminalView's own ResizeObserver already covers ratios.
    expect(terminalCenterSource).toContain(
      "layout.arrangement, layout.maximizedPaneId, layout.panes.length]);");
    expect(terminalCenterSource).not.toMatch(
      /launcherOpen, layout, visible, workspaceView\]/u);
  });

  it("does not transform the terminal surface on arrival, which leaves xterm canvases stale", () => {
    expect(themeCss).toContain(
      ".workspace-surface:not(.is-hidden):has(.terminal-center) {",
    );
    expect(themeCss).toMatch(
      /\.workspace-surface:not\(\.is-hidden\):has\(\.terminal-center\) \{[\s\S]*?animation: none;/u,
    );
  });

  it("paints xterm's viewport with the pane background instead of stock black", () => {
    expect(terminalCss).toContain(".terminal-xterm .xterm { width: 100%; height: auto; flex: none; }");
    expect(terminalCss).toMatch(
      /\.terminal-xterm \.xterm \.xterm-viewport \{[\s\S]*?background-color: var\(--terminal-content-bg\);/u,
    );
  });

  it("refuses to fit hidden or zero-area hosts", () => {
    const visibleParent = {} as HTMLElement;
    expect(terminalHostCanFit({
      offsetParent: visibleParent, clientWidth: 640, clientHeight: 360,
    })).toBe(true);
    expect(terminalHostCanFit({
      offsetParent: null, clientWidth: 640, clientHeight: 360,
    })).toBe(false);
    expect(terminalHostCanFit({
      offsetParent: visibleParent, clientWidth: 0, clientHeight: 360,
    })).toBe(false);
  });

  it("reads a buffer preview backwards, bounded, newest last", () => {
    // A fake buffer rather than a live xterm: the renderer suite has no DOM, and
    // this walk is the part that can go wrong — direction, bound, and order.
    const buffer = (rows: readonly string[]): TerminalTailBuffer => ({
      length: rows.length,
      getLine: (y: number) => y < 0 || y >= rows.length
        ? undefined
        : { translateToString: () => rows[y] },
    });
    const rows = ["oldest", "  ", "middle", "", "newest  "];
    // Oldest first, blanks skipped, and the trailing spaces trimmed.
    expect(terminalTailLines(buffer(rows), 5)).toEqual(["oldest", "middle", "newest"]);
    // The limit counts non-blank rows and takes them from the END, not the start.
    expect(terminalTailLines(buffer(rows), 2)).toEqual(["middle", "newest"]);
    expect(terminalTailLines(buffer(rows), 1)).toEqual(["newest"]);
    // The real buffer-to-card path must retain indentation until the card
    // projection has joined a provider's wrapped reply.
    const wrapped = terminalTailLines(buffer([
      "› Review the retry path", "• The retry handler preserves", "  the request identity.",
      "› Ask Codex to do anything", "  gpt-6-astra · /work/interface",
    ]), 40);
    expect(terminalCardPreview(wrapped, "/work/interface", "codex")).toEqual([
      "› Review the retry path", "• The retry handler preserves the request identity.",
    ]);
    // Rows xterm wrapped are one line again, a space at the wrap point kept.
    const soft = (rows: readonly (readonly [string, boolean])[]): TerminalTailBuffer => ({
      length: rows.length,
      getLine: (y: number) => y < 0 || y >= rows.length ? undefined : {
        translateToString: (trim: boolean) => (trim ? rows[y][0].trimEnd() : rows[y][0]),
        isWrapped: rows[y][1],
      },
    });
    expect(terminalTailLines(soft([
      ["✻ Thinking… (esc to interrupt · 4", false], ["1s · ↓ 2.1k tokens)", true],
      ["the end of ", false], ["a sentence", true], ["", false],
    ]), 2)).toEqual(["✻ Thinking… (esc to interrupt · 41s · ↓ 2.1k tokens)", "the end of a sentence"]);
    // A limit that cannot ask for a line asks for nothing, and never walks.
    for (const limit of [0, -1]) expect(terminalTailLines(buffer(rows), limit)).toEqual([]);
    expect(terminalTailLines(buffer([]), 3)).toEqual([]);
    // The stopAt bound: a cleared alternate screen is thousands of blank rows,
    // and the text above them is out of reach rather than worth the scan.
    const cleared = [
      "scrolled away",
      ...Array.from({ length: TAIL_SCAN_ROWS }, () => ""),
      "prompt",
    ];
    expect(terminalTailLines(buffer(cleared), 5)).toEqual(["prompt"]);
    // One row nearer the end and the same walk does reach it.
    const reachable = ["scrolled away", ...Array.from({ length: TAIL_SCAN_ROWS - 2 }, () => ""),
      "prompt"];
    expect(terminalTailLines(buffer(reachable), 5)).toEqual(["scrolled away", "prompt"]);
    // A row xterm has no line for reads as blank rather than throwing.
    expect(terminalTailLines({ length: 3, getLine: () => undefined }, 2)).toEqual([]);
  });

  it("restores buffered PTY output at its original geometry before enabling fits", () => {
    // Replay order and the setup barrier after the last replayed chunk are
    // behavioral terminal-stream-router tests.
    expect(terminalCenterSource).toContain("initialSize={terminal.size}");
    expect(terminalViewSource).toContain('data-terminal-ready={ready ? "" : undefined}');
    expect(terminalViewSource).toMatch(
      /queueMicrotask\(\(\) => \{[\s\S]*?callbacks\.current\.onReady/u,
    );
    expect(terminalViewSource).toContain("fitReadyRef.current = false");
    expect(terminalViewSource).toContain("if (fitReadyRef.current) tryFit");
  });

  it("SIGWINCHs a pane whose torn agent frame the router did not replay", () => {
    // Which tails replay, and which ask for this repaint, are behavioral
    // terminal-stream-router tests; the bounded tail kept across a remount is
    // there and in terminal-replay-buffer.test.
    expect(terminalCenterSource).toContain("kickRedraw(terminal, size)");
    expect(terminalCenterSource).toContain("terminalRedrawKickSize");
    expect(terminalViewSource).toContain("size() {");
    expect(terminalViewSource).toContain("return { columns: terminal.cols, rows: terminal.rows }");
  });

  it("only reflects the shell Rust hands a finished agent's pane, never deciding one", () => {
    // Protocol 50: Rust decides the hand-off whatever is shown; the renderer
    // mints no shell for an exit and keeps no list of exits it asked for.
    expect(terminalCenterSource).not.toContain("handoffSuppressed");
    expect(terminalCenterSource).not.toContain("handOffToShell");
    const reflect = terminalCenterSource.slice(
      terminalCenterSource.indexOf("for (const shell of registry.terminals) {"),
      terminalCenterSource.indexOf("// `blank`: the ⌘D chords"),
    );
    expect(reflect).toContain("if (shell.replaces === undefined) continue;");
    expect(reflect).toContain("placeHandedOffShell({");
    expect(reflect).toContain("setLayout(current => replaceTerminalTab(current, agentId, shell.id));");
    expect(reflect).not.toContain("createTerminal");
    expect(reflect).not.toContain("dismiss(");
    // A handed-off shell is never seated as a voice or phone start_pane.
    expect(terminalCenterSource).toContain("if (event.terminal.replaces !== undefined) {");
  });

  it("blocks terminal switching and seen-state behind app overlays", () => {
    expect(terminalCenterSource).toContain("visible && !interactionBlocked");
    expect(terminalCenterSource).toContain('event.target.closest(".terminal-center") === null');
  });

  it("splits the focused pane from ⌘D without relaunching its provider", () => {
    expect(terminalCenterSource).toContain("terminalCenterKeyAction(event)");
    expect(terminalCenterSource).toContain(
      'split(paneId, action === "split-horizontal" ? "horizontal" : "vertical", undefined, true);');
    expect(terminalCenterSource).toContain("layoutRef.current.focusedPaneId");
    const splitFn = terminalCenterSource.match(
      /const split = useCallback\([\s\S]*?\n  \}, \[activeCwd, createTerminal, inheritedProfileByTerminal, report\]\);/,
    )?.[0] ?? "";
    expect(splitFn).toContain("const cwd = paneStartDirectory(");
    expect(splitFn).toContain("terminal?.cwd,");
    expect(splitFn).toContain("activeCwd,");
    expect(splitFn).toContain("sessionKey: terminal.sessionKey");
    // The ⌘D chords pass `blank`: no session key and no inherited mark.
    expect(splitFn).toContain("terminal?.sessionKey === undefined || blank");
    expect(splitFn).toContain("terminal !== undefined && !blank");
    // A profile-less shell keeps the session association used for its mark,
    // and records a presentation-only fallback when that association has not
    // arrived yet. Only the pane's + menu can explicitly launch a provider.
    expect(splitFn).not.toContain("launch: terminal.launch");
    expect(splitFn).toContain("launchProfilesRef.current.find");
    expect(splitFn).toContain("terminal.launch?.profileId ?? inheritedProfileByTerminal.get");
    expect(splitFn).toContain("{ inheritedProfileId }");
    expect(terminalCenterSource).toContain("sessionKey: terminal.sessionKey");
    expect(terminalCenterSource).toContain(
      "profileId: terminal.launch?.profileId ?? inheritedProfileByTerminal.get(terminal.id)",
    );
    expect(terminalCenterSource).toContain(
      ">(readInheritedTerminalProfiles);",
    );
    expect(terminalCenterSource).toContain(
      "writeInheritedTerminalProfiles(inheritedProfileByTerminal);",
    );
    expect(terminalCenterSource).toContain("if (!registryLoaded || !workspacesLoaded) return;");
    expect(terminalCenterSource).toContain(
      "for (const terminal of workspace.terminals) liveTerminalIds.add(terminal.id);",
    );
    expect(terminalCenterSource).toContain("!claimed.has(terminalId)");
    expect(terminalViewSource).toContain(
      "if (terminalCenterKeyAction(event) !== null) return false;",
    );
  });

  it("hands a running pane's transcript to a fresh launcher on pass off", () => {
    expect(terminalCenterSource).toContain("const passOff = useCallback");
    expect(terminalCenterSource).toContain("passOffRefusal(");
    expect(terminalCenterSource).toContain("passOffTranscript(");
    expect(terminalCenterSource).toContain("onRequestPassOff: passOff");
    // Without this the handoff is built and then dropped on the floor: every
    // assertion above still passes while the new pane opens blank.
    expect(terminalCenterSource).toContain("{ handoff: input.handoff }");
    // No attached view means no transcript to read, and a pass-off carrying an
    // empty one is that same blank pane wearing the feature's name.
    expect(terminalCenterSource).toContain("if (handle === undefined) {");
  });

  it("keeps Shift+Tab in the pane as the program's reverse tab, never a focus move", () => {
    expect(terminalViewSource).toContain("if (terminalReverseTabGoesToProgram(event)) {");
    expect(terminalViewSource).toContain(
      'if (event.type === "keydown") terminal.input(TERMINAL_REVERSE_TAB)');
    // The branch cancels the event itself rather than borrowing the
    // screen-reader-conditional cancel below it, which is what let the
    // browser move focus to another pane on Shift+Tab.
    const branch = terminalViewSource.slice(
      terminalViewSource.indexOf("if (terminalReverseTabGoesToProgram(event)) {"),
      terminalViewSource.indexOf("if (terminalCopyGoesToProgram(event, {"));
    expect(branch).toContain("event.preventDefault();");
    expect(branch).not.toContain("screenReaderMode !== true) event.preventDefault()");
  });

  it("adds macOS line deletion and native prompt cursor placement without replaying Alt-click", () => {
    expect(terminalViewSource).toContain("const input = terminalInputForKey(event)");
    expect(terminalViewSource).toContain('if (event.type === "keydown") terminal.input(input)');
    expect(terminalViewSource).toContain(
      "if (terminal.options.screenReaderMode !== true) event.preventDefault()",
    );
    expect(terminalViewSource).not.toContain("terminalPlainClickMovesCursor");
    expect(terminalViewSource).not.toContain("replayTerminalMouseUpAsAltClick");
    expect(terminalViewSource).not.toContain('new view.MouseEvent("mouseup"');
    expect(terminalViewSource).toContain('installTerminalCursorPlacement(terminal, () => cursorState.current)');
    expect(terminalCenterSource).toContain("cursorProfile={engine === 'codex' || engine === 'claude' ? engine : undefined}");
  });

  it("draws nothing over the text: search, copy and paste wait for the pane head's request", () => {
    const html = renderToStaticMarkup(createElement(TerminalView, {
      terminalId: "terminal-a",
      active: true,
      onData: () => undefined,
      onCopy: () => undefined,
      onPasteRequest: () => "pasted",
      onDrop: () => undefined,
    }));
    expect(html).toContain('data-terminal-id="terminal-a"');
    expect(html).toContain('aria-label="Interactive terminal"');
    // The floating strip of Search / Copy / Paste is gone (owner,
    // 2026-11-15): the head's ··· menu's Copy and Paste ask the shown
    // emulator through one event, and ⌘F, ⌘C and ⌘V work as before.
    expect(html).not.toContain('aria-label="Search terminal"');
    expect(html).not.toContain('aria-label="Copy selection"');
    expect(html).not.toContain('role="search"');
    expect(terminalViewSource).toContain("host.addEventListener(TERMINAL_VIEW_COMMAND_EVENT, command);");
    expect(terminalViewSource).toContain("host.removeEventListener(TERMINAL_VIEW_COMMAND_EVENT, command);");
    expect(terminalViewSource).toMatch(/name === "find"\) setSearchOpen/u);
    expect(terminalViewSource).toMatch(/name === "copy"\) handle\.copySelection\(\)/u);
    expect(terminalViewSource).toMatch(/name === "paste"\) void handle\.requestPaste\(\)/u);
    expect(terminalViewSource).toContain("if (terminalOpensSearch(event, /Mac/i.test(navigator.platform)))");
    expect(terminalViewSource).not.toContain('className="terminal-search is-floating"');
    // Clearing is unconfirmed and cannot be undone, so it is behind the pane's
    // ··· rather than a trash can in a strip that appears under the pointer.
    expect(html).not.toContain('aria-label="Clear terminal scrollback"');
    expect(html).toContain('class="terminal-xterm"');
    expect(html).not.toContain("drop-indicator");
  });

  it("shows every attachment drop as one glow-and-pill cue saying what the drop does", () => {
    expect(terminalViewSource).toContain("nextDropHoverCount");
    expect(terminalViewSource).toContain("promptDropAccepts");
    expect(terminalViewSource).toContain('{dropHover > 0 && (\n          <DropIndicator label="Drop to insert path" />');
    const read = (name: string): string => readFileSync(new URL(name, import.meta.url), "utf8");
    for (const surface of ["./Conversation.tsx", "./Composer.tsx"]) {
      expect(read(surface)).toContain('<DropIndicator label="Drop files to attach" />');
    }
    expect(renderToStaticMarkup(createElement(DropIndicator, { label: "Drop to insert path" }))).toBe(
      '<div class="drop-indicator" aria-hidden="true"><span class="drop-indicator-pill">'
      + '<span class="codicon codicon-file"></span>Drop to insert path</span></div>');
    const css = read("./DropIndicator.css");
    // A glow behind the pill, not a wash or a dashed box hiding the content.
    expect(css).toMatch(/\.drop-indicator \{[^}]*pointer-events: none;[^}]*background: radial-gradient\([^;]*var\(--terminal-drop-bg\), transparent\);/u);
    expect(css).toMatch(/\.drop-indicator-pill \{[^}]*border-radius: var\(--radius-pill\);/u);
    expect(css).not.toMatch(/^\s*border(?:-style)?:/mu);
    expect(read("./Conversation.css")).not.toContain("conv-drop");
    expect(read("./Composer.css")).not.toContain("composer-drop");
  });
});

describe("pane delegation retired (2026-10-14)", () => {
  it("draws each pane in a plain frame, with no delegated-work item and no worker poll", () => {
    const pane = { id: "pane-x", tabs: [{ terminalId: "x" }], activeTerminalId: "x" };
    const terminal = { id: "x", title: "Claude Code", cwd: "/tmp", status: "running" as const,
      profileId: "claude", exitCode: null };
    const html = renderToStaticMarkup(createElement(TerminalPane, {
      pane: pane as never, terminals: new Map([[terminal.id, terminal as never]]),
      focused: true, maximized: false, onFocus: () => undefined, onSelectTab: () => undefined,
      onCloseTab: () => undefined, onMoveTerminal: () => undefined, onRequestSplit: () => undefined,
      onToggleMaximize: () => undefined, renderTerminal: () => createElement("span"),
    }));
    expect(html.startsWith('<div class="terminal-pane-frame"><section')).toBe(true);
    const menu = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      onClear: () => undefined, onInspectTask: () => undefined,
    }));
    const items = [...menu.matchAll(/role="menuitem"[^>]*>(?:<span[^>]*><\/span>)?<span>([^<]*)<\/span>/gu)]
      .map(([, label]) => label);
    expect(items.length).toBeGreaterThan(0);
    expect(items.some(label => /deleg/iu.test(label))).toBe(false);
    expect(terminalCenterSource).not.toContain("readTerminalDelegation");
    expect(terminalCss).not.toContain("delegation");
  });
});

describe("pane head is the tabs and the actions, nothing else (owner, 2026-11-15)", () => {
  const head = (terminals: readonly TerminalPaneTerminal[], extra: Record<string, unknown> = {}): string => {
    const pane = { id: "pane-h", tabs: terminals.map(terminal => ({ terminalId: terminal.id })),
      activeTerminalId: terminals[0].id };
    return renderToStaticMarkup(createElement(TerminalPane, {
      pane: pane as never, terminals: new Map(terminals.map(terminal => [terminal.id, terminal])),
      focused: true, maximized: false, onFocus: () => undefined, onSelectTab: () => undefined,
      onCloseTab: () => undefined, onMoveTerminal: () => undefined, onRequestSplit: () => undefined,
      onToggleMaximize: () => undefined, onOpenWorkspaceTool: () => undefined,
      renderTerminal: () => createElement("span"), ...extra,
    }));
  };
  const shell: TerminalPaneTerminal = { id: "s", title: "Terminal", cwd: "/Users/ann/Projects/atlas",
    status: "running", activity: "idle", exitCode: null };

  it("draws no folder, no state in words, no Find and no Files: a tab's dot is its state", () => {
    const agent: TerminalPaneTerminal = { ...shell, id: "a", title: "Claude Code", profileId: "claude",
      activity: "working" };
    const html = head([agent]);
    const header = html.slice(html.indexOf("<header"), html.indexOf("</header>"));
    expect(header).toContain('<header class="terminal-pane-head"');
    // The working agent's state is the dot on its own tab, the active tab included.
    expect(header).toContain('<span class="terminal-tab-status is-working" aria-hidden="true"></span>');
    expect(header).toContain('<span class="terminal-tab-title">Claude Code</span>');
    // The only words of state are the ones assistive tech reads off the tab.
    expect(header.match(/Working/gu)).toHaveLength(1);
    expect(header).toContain('<span class="terminal-sr-only">, Working</span>');
    for (const gone of ["terminal-pane-meta", "terminal-pane-folder", "terminal-pane-status",
      "terminal-pane-word", ">atlas<", " min<", 'aria-label="Find in terminal"',
      'aria-label="Files in this folder"']) {
      expect(header, gone).not.toContain(gone);
    }
    // An idle shell says nothing and draws no dot (owner, 2026-09-12).
    const idle = head([shell]);
    const idleHeader = idle.slice(idle.indexOf("<header"), idle.indexOf("</header>"));
    expect(idleHeader.match(/Idle/gu)).toHaveLength(1);
    expect(idleHeader).toContain('<span class="terminal-sr-only">, Idle</span>');
    // The icon actions follow the tabs directly, in their order.
    const at = (label: string): number => header.indexOf(`aria-label="${label}"`);
    expect(at("More pane actions")).toBeGreaterThan(header.indexOf("</div>"));
    expect(at("More pane actions")).toBeLessThan(at("Maximize terminal pane"));
    expect(at("Maximize terminal pane")).toBeLessThan(at("New terminal in this pane"));
    expect(at("New terminal in this pane")).toBeLessThan(at("Close active terminal (ends it)"));
    // Nothing hides the active tab's dot, and no step of a narrow head is
    // left over from the words.
    expect(terminalCss).not.toContain(".terminal-tab-wrap.is-active .terminal-tab-status");
    for (const rule of [".terminal-pane-meta", ".terminal-pane-folder", ".terminal-pane-status",
      ".terminal-pane-word", ".terminal-pane-head.is-single"]) {
      expect(terminalCss, rule).not.toContain(rule);
    }
  });

  it("keeps the amber Needs attention word for any tab blocked on you, the active one included", () => {
    const waiting: TerminalPaneTerminal = { ...shell, id: "a", title: "Claude Code", profileId: "claude",
      activity: "waiting" };
    const word = '<span class="terminal-pane-needs" aria-hidden="true">Needs attention</span>';
    expect(head([waiting])).toContain(word);
    expect(head([shell, waiting])).toContain(word);
    expect(head([shell])).not.toContain("terminal-pane-needs");
    expect(head([{ ...waiting, activity: "working" }])).not.toContain("terminal-pane-needs");
  });

  it("gives a Browser tab the same head", () => {
    const page: TerminalPaneTerminal = { id: "browser:1", title: "Example", cwd: "/Users/ann/Projects/atlas",
      url: "https://example.com", status: "running", activity: "idle", exitCode: null, profileId: "browser" };
    const html = head([page]);
    const header = html.slice(html.indexOf("<header"), html.indexOf("</header>"));
    expect(header).not.toContain('aria-label="Find in terminal"');
    expect(header).not.toContain('aria-label="Files in this folder"');
    expect(header).toContain('aria-label="More pane actions"');
  });

  it("puts Copy and Paste first in the ··· menu, Copy dead until text is selected", () => {
    const tabStops = (markup: string): string[] =>
      [...markup.matchAll(/role="menuitem"[^>]*?tabindex="(-?\d)"/gu)].map(([, value]) => value);
    const labels = (markup: string): string[] =>
      [...markup.matchAll(/role="menuitem"[^>]*>(?:<span[^>]*><\/span>)?<span>([^<]*)<\/span>/gu)].map(([, label]) => label);
    const menu = (copyEnabled: boolean): string => renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      onCopy: () => undefined, onPaste: () => undefined, copyEnabled,
      onClear: () => undefined,
    }));
    expect(labels(menu(true))).toEqual(["Copy", "Paste", "Split right", "Split down", "Clear scrollback"]);
    expect(tabStops(menu(true))).toEqual(["0", "-1", "-1", "-1", "-1"]);
    expect(menu(true)).toMatch(/aria-label="Copy selection" aria-keyshortcuts="Meta\+C" title="Copy \(⌘C\)"/u);
    expect(menu(true)).toMatch(/aria-label="Paste into terminal" aria-keyshortcuts="Meta\+V" title="Paste \(⌘V\)"/u);
    expect(menu(false)).toMatch(/aria-label="Copy selection" aria-keyshortcuts="Meta\+C" disabled=""/u);
  });

  it("routes Find, Copy and Paste to the active tab's emulator through one event", () => {
    const received: string[] = [];
    const host = { dispatchEvent: (event: Event) => {
      received.push(`${event.type}:${String((event as CustomEvent).detail)}`);
      return true;
    }, closest: () => ({ hasAttribute: (name: string) => name === "data-terminal-selection" }) };
    const pane = { querySelector: (selector: string) => {
      expect(selector).toBe(".terminal-tabpanel.is-active .terminal-view .terminal-xterm");
      return host;
    } } as unknown as ParentNode;
    expect(sendTerminalViewCommand(pane, "find")).toBe(true);
    expect(sendTerminalViewCommand(pane, "paste")).toBe(true);
    expect(received).toEqual([`${TERMINAL_VIEW_COMMAND_EVENT}:find`, `${TERMINAL_VIEW_COMMAND_EVENT}:paste`]);
    expect(terminalViewHasSelection(pane)).toBe(true);
    const empty = { querySelector: () => null } as unknown as ParentNode;
    expect(sendTerminalViewCommand(empty, "copy")).toBe(false);
    expect(terminalViewHasSelection(empty)).toBe(false);
  });
});
