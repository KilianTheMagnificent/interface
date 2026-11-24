import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Composer, type ComposerThreadCard } from "./components/Composer";
import type { EngineChoices, EngineOption } from "./composer-model";
import { contrastRatio } from "./contrast";
import { NO_PROJECT_HUE_CLASS, projectHueClass } from "./project-hue";
import type { ThreadWorkspaceChoice } from "./thread-card";
import type { ProviderAccountsView } from "../shared/provider-accounts";

/* ==========================================================================
   THE NEW THREAD CARD, as markup (owner, 2026-10-23: the Code mode look).
   One column: the question beside the still thinking mark, the chips that
   say where the thread runs (the project, and a project's workspace), the
   prompt card with its footer of chips, and the hints below. "No project" is
   the chat card exactly (chat engines only, no access mode, no workspace); a
   project adds the workspace, every engine and the access mode. The card
   without the `thread` prop is pinned unchanged by composer.test.ts and
   ui-components.test.ts.
   ========================================================================== */

const CLAUDE: EngineChoices = {
  models: ["claude-opus-5"],
  modelOptions: [{ value: "claude-opus-5", displayName: "Opus 5", supportsEffort: false, effortLevels: [] }],
  efforts: [], permissionModes: ["default", "auto"], chat: true,
};
const CODEX: EngineChoices = {
  models: ["gpt-6"],
  modelOptions: [{ value: "gpt-6", displayName: "GPT-6", supportsEffort: false, effortLevels: [] }],
  efforts: [], permissionModes: ["untrusted", "never"],
};
const ENGINES: EngineOption[] = [{ id: "claude" }, { id: "codex" }];
const choicesFor = (engine: string): EngineChoices => engine === "codex" ? CODEX : CLAUDE;

const ROOT = "/Users/owner/Developer/Projects/atlas";

function thread(over: Partial<ComposerThreadCard> = {}): ComposerThreadCard {
  return {
    projects: [{ root: ROOT, label: "atlas" }],
    project: ROOT,
    onProjectChange: () => undefined,
    onChooseFolder: () => Promise.resolve(null),
    workspace: { mode: "local" },
    onWorkspaceChange: () => undefined,
    branches: { current: "main", defaultBranch: "main", branches: ["main", "release/0.4"] },
    worktrees: [{ path: ROOT }, { path: "/Users/owner/Developer/Projects/atlas-hotfix", branch: "hotfix" }],
    worktreeUnavailable: null,
    error: null,
    busy: false,
    onLaunch: () => Promise.resolve(true),
    ...over,
  };
}

function card(over: Partial<ComposerThreadCard> = {}, engine = "claude", model = "claude-opus-5"): string {
  return renderToStaticMarkup(createElement(Composer, {
    open: true,
    kind: "chat",
    defaults: { engine, cwd: "", prompt: "", model, effort: "", permissionMode: "default" },
    engines: ENGINES,
    choicesFor,
    onLaunch: () => undefined,
    onBrowse: () => Promise.resolve(null),
    onClose: () => undefined,
    thread: thread(over),
  })).replace(/<style[^>]*>[\s\S]*?<\/style>/gu, "");
}

describe("the New thread card", () => {
  it("asks what to build in the project beside the still mark, the picker in the chips below", () => {
    const markup = card();
    const headline = markup.slice(markup.indexOf("composer-thread-headline"),
      markup.indexOf("</h1>"));
    expect(headline).toContain("What should we build in");
    expect(headline).toContain('<span class="composer-thread-name">atlas</span>');
    expect(headline).toContain('class="composer-thread-trail">?<');
    // The Pair, at rest and decorative, beside the question.
    expect(headline).toMatch(/<svg class="thinking-mark is-still composer-thread-mark" width="36" height="36"[^>]*aria-hidden="true"/u);
    // The picker is the first of the chips under the question, not in it.
    expect(headline).not.toContain('aria-label="Project:');
    const chips = markup.indexOf('class="composer-thread-chips"');
    expect(chips).toBeGreaterThan(markup.indexOf("</h1>"));
    expect(markup.indexOf('aria-label="Project: atlas"')).toBeGreaterThan(chips);
    // One column: the question, the chips, then the prompt; no wordmark hero, no dock pill.
    expect(chips).toBeLessThan(markup.indexOf("composer-ta"));
    expect(markup).not.toContain("composer-intro");
    expect(markup).not.toContain("Start a thread");
    expect(markup).toContain('class="composer-card composer-thread-card"');
    // The repository button is the picker's job now.
    expect(markup).not.toContain('aria-label="Choose repository"');
    expect(markup).toContain('placeholder="Describe the task. @ mentions a file, / runs a command."');
    expect(markup).toContain('rows="3"');
  });

  it("is today's chat card with No project: an open question, no drawer, no access mode", () => {
    const markup = card({ project: null });
    expect(markup).toContain("What’s on your mind?");
    expect(markup).toContain('aria-label="Project: No project"');
    expect(markup).not.toContain("composer-thread-trail");
    expect(markup).not.toContain("composer-thread-drawer");
    expect(markup).not.toContain('aria-label="Access mode');
    expect(markup).not.toContain("Session options");
    expect(markup).toContain(">Start chat<");
    expect(markup).toContain('placeholder="Ask anything."');
    // Already without a project: the way out of one is not offered.
    expect(markup).not.toContain("or chat without a project");
    expect(markup).toContain("start and write another");
  });

  it("moves an engine that cannot chat onto a chat engine for No project", () => {
    const markup = card({ project: null }, "codex", "gpt-6");
    expect(markup).toContain("Opus 5");
    expect(markup).not.toContain("GPT-6");
  });

  it("puts the footer's chips in one row: access by the +, then model, effort and Start", () => {
    const markup = card({}, "codex", "gpt-6");
    expect(markup).toContain("GPT-6");
    // The model chip names the model; its provider is the logo beside it.
    expect(markup).toContain('aria-label="Model: GPT-6"');
    expect(markup).toContain('aria-label="Access mode');
    // The access chip leads with the Threads access shield, not the codicon.
    const access = markup.slice(markup.indexOf('aria-label="Access mode'));
    expect(access.slice(0, access.indexOf("</button>"))).toContain('class="thread-glyph"');
    expect(markup).not.toContain("codicon-shield");
    // Session options stays reachable, as a gear named for what it opens.
    expect(markup).toMatch(/aria-label="Session options"[^>]*>[^<]*<span class="codicon codicon-sm codicon-settings-gear"/u);
    expect(markup).toContain(">Start thread<");
    // The canvas's order: + and access on the left, the model chips and Start on the right.
    expect(markup.indexOf('aria-label="Add prompt context')).toBeLessThan(markup.indexOf('aria-label="Access mode'));
    expect(markup.indexOf('aria-label="Access mode')).toBeLessThan(markup.indexOf('aria-label="Model:'));
    expect(markup.indexOf('aria-label="Model:')).toBeLessThan(markup.indexOf('class="composer-submit"'));
    const secondary = markup.slice(markup.indexOf("composer-control-secondary"));
    expect(secondary).toContain('aria-label="Model: GPT-6"');
    expect(secondary).not.toContain('aria-label="Access mode');
    // Start is the accent square with the Threads arrow; no hairlines split the row.
    expect(markup).toMatch(/class="composer-submit"[^>]*><svg class="thread-glyph composer-submit-glyph"/u);
    expect(markup).not.toContain("composer-thread-sep");
    // The footer and the hints below the card.
    expect(markup).toContain(">or chat without a project<");
    expect(markup).toMatch(/<kbd>↵<\/kbd> start[\s\S]*<kbd>⌘↵<\/kbd> start and write another/u);
  });

  it("puts an account control beside the model only when the provider has several accounts", () => {
    const accounts: ProviderAccountsView = { version: 1, providers: [
      { provider: "claude", engine: "claude", accounts: [
        { id: "default", label: "Default", isDefault: false, signIn: "signedIn" },
        { id: "acct-0123456789ab", label: "Work", isDefault: true, signIn: "signedIn", directory: "/a/w" },
      ] },
      { provider: "codex", engine: "codex", accounts: [
        { id: "default", label: "Default", isDefault: true, signIn: "unknown" },
      ] },
    ] };
    const several = card({ accounts });
    // Starts on the provider's default account, right after the model chip.
    expect(several).toContain('aria-label="Account: Work account"');
    expect(several.indexOf('aria-label="Model:')).toBeLessThan(several.indexOf('aria-label="Account: Work account"'));
    expect(card({ accounts }, "codex", "gpt-6"), "one account").not.toContain('aria-label="Account');
    expect(card(), "no accounts read").not.toContain('aria-label="Account');
  });

  it("puts a project's workspace beside it, above the prompt: Local names the checked-out branch", () => {
    const markup = card();
    const drawer = markup.slice(markup.indexOf("composer-thread-drawer"));
    expect(markup.indexOf("composer-thread-drawer")).toBeGreaterThan(markup.indexOf('aria-label="Project: atlas"'));
    expect(markup.indexOf("composer-thread-drawer")).toBeLessThan(markup.indexOf("composer-box"));
    expect(drawer).toContain('role="group" aria-label="Workspace"');
    expect(drawer).toMatch(/aria-pressed="true"[^>]*>.*Local checkout/u);
    expect(drawer).toMatch(/aria-pressed="false"[^>]*>.*New worktree/u);
    expect(drawer).toContain("Edits land in atlas itself.");
    // Read-only: Local works on what the project has checked out.
    expect(drawer).toContain('class="composer-thread-branch-name">main<');
    expect(drawer).not.toContain("composer-branch-picker");
  });

  it("offers the base branch for a new worktree", () => {
    const worktree: ThreadWorkspaceChoice = { mode: "worktree", baseRef: "release/0.4" };
    const markup = card({ workspace: worktree });
    expect(markup).toContain("composer-branch-picker");
    expect(markup).toContain('aria-label="Base branch: release/0.4"');
    expect(markup).toMatch(/aria-pressed="true"[^>]*>.*New worktree/u);
    expect(markup).toContain("A new branch in its own folder.");
    expect(markup).not.toContain("composer-thread-branch-name");
    // The current branch row names the branch, not "From …".
    expect(card({ workspace: { mode: "worktree" } })).toContain('aria-label="Base branch: main"');
  });

  it("keeps an existing worktree a segment away, and shows a chosen one by its branch", () => {
    // Offered whenever the project has another worktree.
    expect(card()).toContain('aria-label="Existing worktree: Existing…"');
    expect(card({ worktrees: [{ path: ROOT }] })).not.toContain("composer-thread-existing");
    const markup = card({ workspace: { mode: "existing",
      worktreePath: "/Users/owner/Developer/Projects/atlas-hotfix" } });
    expect(markup).toContain("composer-thread-existing is-on");
    expect(markup).toContain(">hotfix<");
    expect(markup).toContain("Edits land in a worktree that already exists.");
  });

  it("says why there is no new worktree in a folder that is not a git repository", () => {
    const markup = card({ worktreeUnavailable: "Not a git repository: /tmp/x" });
    expect(markup).toMatch(/aria-disabled="true" title="Not a git repository: \/tmp\/x"[^>]*>.*New worktree/u);
  });

  it("shows Rust's refusal above the card and holds the launch while one runs", () => {
    const refused = card({ error: "Not a git repository: /tmp/x" });
    expect(refused).toContain("Not a git repository: /tmp/x");
    expect(refused.indexOf("Not a git repository")).toBeLessThan(refused.indexOf("composer-box"));
    const busy = card({ busy: true });
    expect(busy).toContain(">Starting…<");
    expect(busy).toContain('aria-busy="true"');
  });

  it("says in the card that Edit from the first message restored the files", () => {
    // The card covers the pane, and the pane's recovery notice with it.
    const markup = card({ workspace: { mode: "existing",
      worktreePath: "/Users/owner/Developer/Projects/atlas-hotfix" },
    note: "Files restored to before this message — recovery point saved" });
    expect(markup).toMatch(/role="status"[^>]*>Files restored to before this message — recovery point saved</u);
    expect(card()).not.toContain("composer-thread-note");
  });

  it("drops the per-launch worktree checkbox: the workspace control owns it", () => {
    expect(card()).not.toContain('type="checkbox"');
    expect(card()).not.toContain("Run in an isolated Git worktree");
  });
});

/* ==========================================================================
   THE CODE MODE LOOK (owner, 2026-10-23). The hue glow is gone from the card:
   no aurora behind the question, no tinted picker, no gradient edge or glow
   on the prompt, no hued Start or pressed segment. A project keeps its hue
   only as the 8px square in its picker; No project is a hollow ring. The
   prompt is the Threads composer's card and Start the accent square.
   ========================================================================== */

const composerCss = readFileSync(new URL("./components/Composer.css", import.meta.url), "utf8");
const themeCss = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
/** The New thread card's own section of the sheet. */
const cardCss = composerCss.slice(composerCss.indexOf("/* ---- the New thread card"),
  composerCss.indexOf("@keyframes composer-arrive"));

/** Every declaration of the rules that list exactly this selector. */
function rulesFor(selector: string): string {
  return [...composerCss.replace(/\/\*[\s\S]*?\*\//gu, "").matchAll(/([^{}]+)\{([^{}]*)\}/gu)]
    .filter(([, selectors]) => selectors.split(",").map(item => item.trim()).includes(selector))
    .map(([, , body]) => body.replace(/\s+/gu, " ").trim()).join(" ");
}

function themeToken(theme: "light" | "dark", name: string): string {
  const block = themeCss.match(new RegExp(`:root\\[data-theme="${theme}"\\] \\{([\\s\\S]*?)\\n\\}`, "u"))?.[1] ?? "";
  const value = block.match(new RegExp(`--${name}:\\s*([^;]+);`, "u"))?.[1].trim();
  if (value === undefined) throw new Error(`No --${name} in the ${theme} theme`);
  return value;
}

describe("the New thread card in the Code mode look", () => {
  it("keeps a project's hue only as the picker's square, and No project a ring", () => {
    const markup = card();
    const picker = markup.slice(markup.indexOf('aria-label="Project: atlas"'));
    expect(picker.slice(0, picker.indexOf("</button>"))).toContain(
      `<span class="app-select-leading" aria-hidden="true"><span class="composer-thread-hue ${projectHueClass(ROOT)}" aria-hidden="true"></span></span>`);
    expect(projectHueClass(ROOT)).toMatch(/^project-hue-[0-7]$/u);
    expect(card({ project: null })).toContain(`class="composer-thread-hue ${NO_PROJECT_HUE_CLASS}"`);
    // The card itself wears no hue class, and no letter tile is drawn.
    expect(markup).not.toMatch(/composer-thread-card project-hue-/u);
    expect(markup).not.toContain("project-tile");
    const square = rulesFor(".composer-thread-hue");
    expect(square).toContain("width: 8px");
    expect(square).toContain("height: 8px");
    expect(square).toContain("background: var(--project-h)");
    expect(square).toContain("border-radius: calc(var(--radius-control) * .625)");
    const ring = rulesFor(".composer-thread-hue.project-hue-none");
    expect(ring).toContain("background: transparent");
    expect(ring).toContain("border-radius: 50%");
  });

  it("draws no aurora, no hued edge, picker or Start", () => {
    expect(card()).not.toContain("composer-thread-aurora");
    expect(card({ project: null })).not.toContain("composer-thread-aurora");
    expect(cardCss).not.toContain("composer-thread-aurora");
    expect(cardCss).not.toContain("--project-h2");
    expect(cardCss).not.toContain("linear-gradient");
    expect(cardCss).not.toContain("radial-gradient");
    // The square is the one rule in the card's section that reads the hue.
    const hued = [...cardCss.replace(/\/\*[\s\S]*?\*\//gu, "").matchAll(/([^{}]+)\{([^{}]*)\}/gu)]
      .filter(([, , body]) => body.includes("--project-h")).map(([, selectors]) => selectors.trim());
    expect(hued).toEqual([".composer-thread-hue"]);
  });

  it("asks its question in the serif, 32px, beside the still mark", () => {
    const greeting = rulesFor(".composer-thread-headline");
    expect(greeting).toContain("font-size: calc(var(--text-display) * 16 / 11)");
    expect(greeting).toContain("font-weight: var(--wght-regular)");
    expect(greeting).toContain("gap: 16px");
    // Bare CLI keeps its one terminal face.
    expect(rulesFor(':where(:root:not([data-chrome="cli"])) .composer-thread-headline'))
      .toContain("font-family: var(--font-ui-serif)");
  });

  it("draws the prompt as the Threads composer's card, keeping the focus edge", () => {
    const box = rulesFor(".composer-thread-card .composer-box");
    expect(box).toContain("background: var(--surface)");
    expect(box).toContain("border-color: var(--border-strong)");
    expect(box).toContain("border-radius: calc(var(--radius-composer) * 11 / 7)");
    expect(rulesFor(':where(:root:not([data-chrome="cli"])) .composer-thread-card .composer-box'))
      .toContain("0 8px 24px color-mix(in srgb, var(--composer-lift-color) 75%, transparent)");
    // Keyboard focus on the prompt is still the app's field-focus edge.
    expect(rulesFor(".composer-box:has(.composer-ta:focus-visible)")).toContain("border-color: var(--focus)");
  });

  it("presses a workspace segment onto the surface, not into a hue", () => {
    const pressed = rulesFor('.composer-thread-segment[aria-pressed="true"]');
    expect(pressed).toContain("background: var(--surface)");
    expect(pressed).toContain("color: var(--fg-strong)");
  });

  it("starts on the accent square, its arrow legible on it in both kinds of theme", () => {
    const start = rulesFor(".composer-thread-card .composer-submit");
    expect(start).toContain("width: 32px");
    expect(start).toContain("height: 32px");
    expect(start).toContain("border-radius: calc(var(--radius-control) * 2.25)");
    expect(start).not.toContain("background");
    for (const theme of ["dark", "light"] as const) {
      expect(contrastRatio(themeToken(theme, "accent-fg"), themeToken(theme, "accent")), theme)
        .toBeGreaterThanOrEqual(3);
    }
  });

  it("opens the slash menu as a popover under the prompt, Bare CLI keeping its list", () => {
    const popover = rulesFor(':where(:root:not([data-chrome="cli"])) .composer-thread-card .provider-command-menu.is-inline');
    expect(popover).toContain("position: absolute");
    expect(popover).toContain("top: calc(100% + 10px)");
    expect(popover).toContain("border-radius: calc(var(--radius-container) * 2)");
  });
});
