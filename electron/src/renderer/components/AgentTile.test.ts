import { readFileSync } from "node:fs";
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SessionSummary } from "../../shared/engine";
import { AgentTile, type AgentTileProps } from "./AgentTile";
import { ThreadPaneMenuItems } from "./ThreadPaneHead";
import { statusWord } from "./status-words";

const summary: SessionSummary = {
  engine: "claude",
  sessionId: "s1",
  title: "Fix the failing test",
  status: "working",
  cwd: "/repo",
  cloud: false,
  archived: false,
  firstSeen: "2026-08-14T00:00:00.000Z",
  lastActivity: "2026-08-14T00:00:00.000Z",
};

const props = (over: Partial<AgentTileProps> = {}): AgentTileProps => ({
  session: summary,
  selected: false,
  maximized: false,
  live: true,
  engineLabel: "Claude",
  onFocus: () => undefined,
  onToggleMaximize: () => undefined,
  onClose: () => undefined,
  children: null,
  ...over,
});

describe("AgentTile header is the chat title", () => {
  it("does not print model, working state, or a diffstat in the tab", () => {
    const html = renderToStaticMarkup(createElement(AgentTile,
      props({ session: { ...summary, model: "claude-opus-4-6" } })));
    expect(html).toContain('class="workspace-tile-name">Fix the failing test<');
    expect(html).not.toContain("workspace-tile-meta");
    expect(html).not.toContain("workspace-tile-status");
    expect(html).not.toContain("workspace-tile-model");
    expect(html).not.toContain("workspace-tile-diff");
    expect(html).not.toContain("claude-opus-4-6");
    expect(html).not.toContain("+12 −4");
    expect(html).not.toContain('class="workspace-tile-status"');
  });

  it("keeps provider and status in the accessible name only", () => {
    const html = renderToStaticMarkup(createElement(AgentTile, props()));
    expect(html).toContain('aria-label="Fix the failing test — Claude — Working"');
    expect(html).toContain('aria-label="Focus Fix the failing test, Claude, Working"');
    // The provider's mark moved to the composer's model chip (Threads redesign,
    // 2026-10-06): it appears once per pane, and not in the head.
    expect(html).not.toContain("data-provider-logo");
    expect(html).not.toContain(">Claude<");
    for (const status of ["waiting", "working", "failed"] as const) {
      const named = renderToStaticMarkup(createElement(AgentTile,
        props({ session: { ...summary, status }, live: false })));
      expect(named).toContain(
        `aria-label="Fix the failing test — Claude — ${statusWord(status)}"`);
      expect(named).not.toContain("workspace-tile-status");
    }
  });

  it("reads as the title over one line: the project, then where the thread works", () => {
    // Owner, 2026-11-15: the title alone on the first line, then the project's
    // hue square and name, then the pane head's words, on the second.
    const html = renderToStaticMarkup(createElement(AgentTile, props({
      projectLabel: "atlas", projectRoot: "/repo/atlas",
      headDetail: createElement("span", { className: "thread-head" }, "on main"),
    })));
    expect(html).toContain('<button type="button" class="workspace-tile-title" title="Fix the failing test"');
    expect(html).toMatch(/class="workspace-tile-title"[^>]*><span class="workspace-tile-name">Fix the failing test<\/span><\/button>/u);
    expect(html).toContain('<div class="workspace-tile-where"><span class="project-tile project-hue-');
    expect(html).toContain('<span class="workspace-tile-project">atlas</span><span class="thread-head">on main</span>');
    expect(html.indexOf("workspace-tile-name")).toBeLessThan(html.indexOf("workspace-tile-project"));
    expect(html).not.toContain("workspace-tile-crumb");
    // A plain chat says No project beside a hollow ring (project-hue-none).
    const chat = renderToStaticMarkup(createElement(AgentTile, props()));
    expect(chat).toContain('<span class="project-tile project-hue-none is-large" aria-hidden="true">·</span>'
      + '<span class="workspace-tile-project">No project</span>');
  });

  it("shows a title that is still its prompt short, and the whole of it in the tooltip", () => {
    const prompt = "burst write plan.md Draft the release checklist for atlas and then check every link in it";
    const paint = (titleSource?: AgentTileProps["titleSource"]): string => renderToStaticMarkup(
      createElement(AgentTile, props({ session: { ...summary, title: prompt }, titleSource })));
    const short = paint("prompt");
    expect(short).toContain('class="workspace-tile-name">burst write plan.md Draft the release checklist for atlas…<');
    expect(short).toContain(`title="${prompt}"`);
    expect(short).toContain(`aria-label="Focus ${prompt}, Claude, Working"`);
    // A title the owner set or Claude generated is shown as it is.
    expect(paint("user")).toContain(`class="workspace-tile-name">${prompt}<`);
    expect(paint("generated")).toContain(`class="workspace-tile-name">${prompt}<`);
  });
});

describe("AgentTile's title focus ring (owner, 2026-11-15)", () => {
  it("wraps the title alone, so focus draws a whole ring instead of two clipped bars", () => {
    // The title button ran the head's full height, and the pane clipped its
    // ring's top and bottom: two bars, one at each end of the title.
    const css = readFileSync(new URL("./ThreadPaneHead.css", import.meta.url), "utf8");
    const rule = (selector: string): string => {
      const start = css.indexOf(`${selector} {`);
      expect(start, selector).toBeGreaterThan(-1);
      return css.slice(start, css.indexOf("}", start));
    };
    expect(rule(".workspace-agent .workspace-tile-title")).toContain("height: auto;");
    expect(rule(".workspace-agent .workspace-tile-title")).toContain("align-self: flex-start;");
    expect(rule(".workspace-agent .workspace-tile-title:focus-visible"))
      .toContain("outline: var(--focus-ring-w) solid var(--focus);");
    // The head grows to two lines and leaves the ring room above the title.
    expect(rule(".workspace-agent .workspace-tile-head")).toContain("height: auto;");
    expect(rule(".workspace-agent .workspace-tile-head")).toContain("padding: 12px 16px 12px 6px;");
    // The TN Code look (owner, 2026-11-16): a 15px toolbar title, bold.
    expect(rule(".workspace-agent .workspace-tile-name")).toContain("font-size: var(--text-h3);");
    expect(css).toMatch(/\.workspace-agent \.workspace-tile-name \{\s*font-family: var\(--tn-font-display\);\s*font-weight: var\(--wght-heavy\);/u);
  });
});

describe("AgentTile's live head (owner, 2026-10-07)", () => {
  const paint = (status: SessionSummary["status"], over: Partial<AgentTileProps> = {}): string =>
    renderToStaticMarkup(createElement(AgentTile, props({ session: { ...summary, status }, ...over })));

  it("says Needs you, filled, before Changes while the thread waits on its owner", () => {
    const html = paint("waiting", { headActions: createElement("button", { className: "thread-head-changes" }) });
    expect(html).toContain('<span class="workspace-tile-needs" aria-hidden="true">Needs you</span>');
    expect(html.indexOf("workspace-tile-needs")).toBeLessThan(html.indexOf("thread-head-changes"));
    expect(html.indexOf("workspace-tile-actions")).toBeLessThan(html.indexOf("workspace-tile-needs"));
    // The state is said once, by the accessible names, not twice.
    expect(html).toContain(`aria-label="Fix the failing test — Claude — ${statusWord("waiting")}"`);
  });

  it("draws no pill in any other state, and keeps working words out of the tab", () => {
    for (const status of ["working", "idle", "failed"] as const) {
      const html = paint(status);
      expect(html, status).not.toContain("workspace-tile-needs");
      expect(html, status).not.toContain("Needs you");
      expect(html, status).not.toContain(">Working<");
    }
  });

  it("moves the Changes chip only once it has watched its totals change", () => {
    // The first render has nothing to compare with: no cue on the head.
    expect(paint("working")).not.toContain("data-changes-cue");
  });
});

describe("AgentTile controls", () => {
  it("puts the pane's own commands behind one ⋯", () => {
    const html = renderToStaticMarkup(createElement(AgentTile,
      props({ onSplit: () => undefined })));
    expect(html).toContain('aria-label="Fix the failing test pane actions"');
    expect(html).toContain('aria-haspopup="menu"');
    // Split, maximize and close are rows of that menu, not head buttons.
    for (const glyph of ["add", "expand-diagonal", "close"]) {
      expect(html).not.toContain(`codicon-${glyph}"`);
    }
    expect(html).not.toContain("workspace-tile-access");
  });

  const items = (over: Partial<ComponentProps<typeof ThreadPaneMenuItems>> = {}): string =>
    renderToStaticMarkup(createElement(ThreadPaneMenuItems, {
      title: "Fix the failing test", maximized: false, splitLabel: "Open a new pane in this project",
      choose: () => undefined, onToggleMaximize: () => undefined, onClose: () => undefined, ...over,
    }));

  it("offers split, maximize and close in that order", () => {
    const html = items({ onSplit: () => undefined });
    expect(html).toContain('aria-label="Open a new pane in this project"');
    expect(html).toContain('aria-label="Maximize agent pane"');
    expect(html).toContain('aria-label="Close Fix the failing test pane"');
    expect(html.indexOf("codicon-add")).toBeLessThan(html.indexOf("codicon-expand-diagonal"));
    expect(html.indexOf("codicon-expand-diagonal")).toBeLessThan(html.indexOf("codicon-close"));
    expect(items({ maximized: true })).toContain('aria-label="Restore agent pane"');
  });

  it("refuses a fifth pane in words rather than by disappearing", () => {
    const html = items({ onSplit: () => undefined, splitDisabled: true });
    expect(html).toContain('title="Workspace is full (4 panes)"');
    expect(html).toContain(
      'aria-label="Open a new pane in this project — workspace is full (4 panes)"');
    const button = /<button[^>]*>(?:(?!<\/button>)[\s\S])*codicon-add[\s\S]*?<\/button>/
      .exec(html)?.[0];
    expect(button).toBeDefined();
    expect(button).toContain("disabled");
  });

  it("offers Open in terminal after the split, refused in words mid-turn (review C1)", () => {
    const html = items({ onSplit: () => undefined, onOpenInTerminal: () => undefined, terminalRefusal: null });
    expect(html).toContain("<span>Open in terminal</span>");
    expect(html.indexOf("codicon-add")).toBeLessThan(html.indexOf("codicon-terminal"));
    expect(html.indexOf("codicon-terminal")).toBeLessThan(html.indexOf("codicon-expand-diagonal"));
    const busy = items({ onOpenInTerminal: () => undefined, terminalRefusal: "Stop the turn or let it finish first." });
    expect(busy).toMatch(/disabled=""[^>]*title="Stop the turn or let it finish first\."/u);
    expect(items()).not.toContain("Open in terminal");
  });

  it("draws no split row at all when the shell offers none", () => {
    expect(items()).not.toContain("codicon-add");
  });
});
