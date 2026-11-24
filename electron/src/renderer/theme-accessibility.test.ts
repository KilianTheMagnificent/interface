import { existsSync, readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { isResolvedTheme, THEME_KIND } from "../shared/appearance";
import { compositeHex, contrastRatio, parseHexColor } from "./contrast";

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const terminalCss = readFileSync(new URL("./terminal.css", import.meta.url), "utf8");
const component = (name: string): string =>
  readFileSync(new URL(`./components/${name}.tsx`, import.meta.url), "utf8")
  + (existsSync(new URL(`./components/${name}.css`, import.meta.url))
    ? readFileSync(new URL(`./components/${name}.css`, import.meta.url), "utf8") : "");

/** A comment naming a selector must not read as that selector still existing:
    the absence tests below are about RULES, and this sheet explains most of its
    deletions in prose right where the rule used to be.

    Blanked rather than deleted: every character is replaced by a space and
    every newline kept, so an index into the stripped source is still the index
    — and the line — it had in the file. The ramp guards below report file:line. */
const stripComments = (source: string): string =>
  source.replace(/\/\*[\s\S]*?\*\//gu, comment => comment.replace(/[^\n]/gu, " "));
const cssRules = stripComments(css);

const lineOf = (source: string, index: number): number =>
  source.slice(0, index).split("\n").length;

/** `selector { body }` pairs from a stylesheet or a CSS-in-JS template. */
type DeclarationBlock = { selector: string; body: string; index: number; bodyIndex: number };
const parsedSheets = new Map<string, readonly DeclarationBlock[]>();
function declarationBlocks(source: string): readonly DeclarationBlock[] {
  const cached = parsedSheets.get(source);
  if (cached !== undefined) return cached;
  // The equivalent regex repeatedly rescans long JSX runs without a closing
  // brace. Keep the same innermost declaration pairs and source offsets in one
  // pass, so this functional check does not time out under build load.
  const clean = stripComments(source);
  const blocks: DeclarationBlock[] = [];
  let previous = -1;
  let candidate: Omit<DeclarationBlock, "body"> | null = null;
  for (let index = 0; index < clean.length; index++) {
    const char = clean[index];
    if (char !== "{" && char !== "}") continue;
    if (char === "{") {
      candidate = index > previous + 1
        ? { selector: clean.slice(previous + 1, index).trim(), index: previous + 1, bodyIndex: index + 1 }
        : null;
    } else {
      if (candidate !== null) blocks.push({ ...candidate, body: clean.slice(candidate.bodyIndex, index) });
      candidate = null;
    }
    previous = index;
  }
  parsedSheets.set(source, blocks);
  return blocks;
}

/** Every sheet the app paints from: the two stylesheets plus the CSS-in-JS
    template strings, which hold most of the renderer's rules. The vocabulary
    guards below all scan this one surface — a token honoured in theme.css and
    ignored in a component template is not a token, it is a suggestion. */
const rendererSheets: { name: string; source: string }[] = (() => {
  const dir = new URL("./components/", import.meta.url);
  return [
    { name: "theme.css", source: css },
    { name: "terminal.css", source: terminalCss },
    { name: "App.tsx", source: readFileSync(new URL("./App.tsx", import.meta.url), "utf8") },
    { name: "markdown.tsx", source: readFileSync(new URL("./markdown.tsx", import.meta.url), "utf8") },
    ...readdirSync(dir).filter(name => /\.(tsx|css)$/.test(name)).sort()
      .map(name => ({ name: `components/${name}`, source: readFileSync(new URL(name, dir), "utf8") })),
  ];
})();

const lightBlock = css.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";
const darkBlock = css.match(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/u)?.[1] ?? "";
/** The shipped palettes. The two a theme file paints over (custom-dark and
    custom-light) mix their colours from the file's keys; theme-files.test.ts
    holds their defaults to the same bars. */
const namedThemeBlocks = [...css.matchAll(/:root\[data-theme="([^"]+)"\] \{([\s\S]*?)\n\}/gu)]
  .map(([, id, body]) => ({ id, body }))
  .filter(({ id }) => !id.startsWith("custom-"));

function token(block: string, name: string): string {
  const match = block.match(new RegExp(`--${name}:\\s*([^;]+);`, "u"));
  if (match === null) throw new Error(`Missing theme token --${name}`);
  return match[1].trim();
}

function resolvedToken(block: string, name: string, seen = new Set<string>()): string {
  if (seen.has(name)) throw new Error(`Circular theme token --${name}`);
  const value = token(block, name);
  const reference = value.match(/^var\(--([a-z0-9-]+)\)$/u)?.[1];
  if (reference === undefined) return value;
  return resolvedToken(block, reference, new Set([...seen, name]));
}

function resolvedValue(block: string, value: string): string {
  const reference = value.match(/^var\(--([a-z0-9-]+)\)$/u)?.[1];
  return reference === undefined ? value : resolvedToken(block, reference);
}

function mixHex(foreground: string, background: string, weight: number): string {
  const front = parseHexColor(foreground);
  const back = parseHexColor(background);
  const channel = (frontValue: number, backValue: number): string =>
    Math.round(frontValue * weight + backValue * (1 - weight))
      .toString(16).padStart(2, "0");
  return `#${channel(front.r, back.r)}${channel(front.g, back.g)}${channel(front.b, back.b)}`;
}

function expectTextContrast(foreground: string, background: string): void {
  expect(contrastRatio(foreground, background)).toBeGreaterThanOrEqual(4.5);
}

/** Every theme block, keyed for assertion messages. */
const allThemeBlocks = [{ id: ":root", body: lightBlock }, ...namedThemeBlocks];

/** Tokens the app paints as text on a list row, a card or a pane.

    --accent is in this list because it is not only a fill: Conversation's
    `.conv-md-link` and SteeringQueue's `.steering-queue-send` both set
    `color: var(--accent)`.
    --fg-strong is the selected-row and active-tab text tier.

    The four --mode-* tokens are here for the same reason --accent is: they are
    not decoration. access-modes.ts routes each provider access token to one of
    them and theme.css paints the picker's VALUE with it, on the launch card and
    on the live session pill — both of which sit on rows. */
const rowTextTokens = [
  "fg", "fg-strong", "fg-muted", "fg-dim", "accent", "accent-text",
  "status-working", "status-waiting", "status-failed", "danger-fg",
  "diff-added", "diff-removed",
  "mode-plan", "mode-accept", "mode-danger", "mode-auto",
] as const;

/** Tokens the app only ever paints as a dot, rail or fill. Idle and done
    are the statuses with no text tier — idle routes to --fg-dim, and done's
    word is spoken and printed in the ordinary text colour — so they answer to
    WCAG 1.4.11's 3:1 graphical floor rather than to AA. */
const rowGraphicTokens = ["status-idle", "status-done"] as const;

/** Resolve a token that may be an #rrggbbaa wash over the chrome sheet. */
function surfaceHex(block: string, name: string, base: string): string {
  const raw = token(block, name);
  return raw.length === 9 ? compositeHex(raw, base) : raw;
}

/** Every background a row-level token actually lands on, hover and selected
    included — the composited states, not just the resting one.

    --row-hover and --row-selected are translucent in four shipped themes, so
    the colour they composite to depends on the sheet underneath. Sidebar rows
    sit on chrome; conversation rows sit on the editor sheet. Both composites
    are measured, because a theme can pass one and fail the other. --surface is
    the panel/card sheet and was missing entirely. */
function rowSurfaces(block: string): Array<{ name: string; hex: string }> {
  const chrome = token(block, "chrome-bg");
  const editor = token(block, "editor-bg");
  return [
    { name: "chrome-bg", hex: chrome },
    { name: "editor-bg", hex: editor },
    { name: "surface", hex: token(block, "surface") },
    { name: "surface-raised", hex: token(block, "surface-raised") },
    { name: "row-hover over chrome-bg", hex: surfaceHex(block, "row-hover", chrome) },
    { name: "row-selected over chrome-bg", hex: surfaceHex(block, "row-selected", chrome) },
    { name: "row-hover over editor-bg", hex: surfaceHex(block, "row-hover", editor) },
    { name: "row-selected over editor-bg", hex: surfaceHex(block, "row-selected", editor) },
  ];
}

describe("theme accessibility contracts", () => {
  it("keeps light text tokens legible on base, hover, and selected surfaces", () => {
    const selected = token(lightBlock, "row-selected");
    const editor = token(lightBlock, "editor-bg");
    expectTextContrast(token(lightBlock, "accent-text"), selected);
    expectTextContrast(token(lightBlock, "fg-selected-secondary"), selected);
    expectTextContrast(token(lightBlock, "fg-dim"), token(lightBlock, "row-hover"));
    expectTextContrast(token(lightBlock, "input-placeholder"), editor);
    expectTextContrast(token(lightBlock, "danger-fg"), token(lightBlock, "danger-bg"));
    expectTextContrast(token(lightBlock, "accent-fg"), token(lightBlock, "accent"));
  });

  it("keeps dark text tokens legible on base, hover, selected, and danger surfaces", () => {
    const chrome = token(darkBlock, "chrome-bg");
    const selected = compositeHex(token(darkBlock, "row-selected"), chrome);
    expectTextContrast(token(darkBlock, "accent-text"), selected);
    expectTextContrast(token(darkBlock, "fg-selected-secondary"), selected);
    expectTextContrast(token(darkBlock, "fg-dim"), token(darkBlock, "surface-raised"));
    expectTextContrast(token(darkBlock, "input-placeholder"), chrome);
    expectTextContrast(token(darkBlock, "danger-fg"), token(darkBlock, "danger-bg"));
    expectTextContrast(token(darkBlock, "accent-fg"), token(darkBlock, "accent"));
  });

  it("separates interaction, review, identity, and status hues without losing contrast", () => {
    for (const body of [lightBlock, darkBlock]) {
      expect(token(body, "selection-accent")).toBe(token(body, "focus"));
      expect(token(body, "badge-bg")).toBe(token(body, "selection-accent"));
      expectTextContrast(token(body, "focus-contrast"), token(body, "selection-accent"));
      expect(token(body, "selection-accent")).not.toBe(token(body, "accent"));
      expect(token(body, "selection-accent")).not.toBe(token(body, "status-working"));
    }
    expect(token(lightBlock, "review-accent")).toBe("var(--mode-auto)");
    for (const { id, body } of namedThemeBlocks.filter(block => !["light", "dark"].includes(block.id))) {
      expect(token(body, "selection-accent"), id).toBe("var(--accent-text)");
    }
  });

  it("keeps chromatic selection labels legible on their neutral pill surface", () => {
    for (const { id, body } of allThemeBlocks) {
      const selection = resolvedToken(body, "selection-accent");
      const selected = surfaceHex(body, "row-selected", token(body, "chrome-bg"));
      expect(
        contrastRatio(selection, token(body, "surface-raised")),
        `${id} --selection-accent on --surface-raised`,
      ).toBeGreaterThanOrEqual(4.5);
      expectTextContrast(selection, selected);
      expectTextContrast(resolvedToken(body, "focus-contrast"), selection);
      expectTextContrast(
        resolvedToken(body, "badge-fg"), resolvedToken(body, "badge-bg"),
      );
    }
    expect(css).toContain("--selection-accent: #4057b2");
    expect(css).toContain("--selection-accent: #8da8f0");
  });

  it("reads every --badge-bg fill against --badge-fg, in the panels too", () => {
    // The pair above is measured per theme; this is the half that was missing.
    // Three pills in the collaboration and orchestration sheets took the fill
    // and then set --fg-dim over it, or set nothing at all and inherited --fg:
    // 1.01:1 in the light theme, never better than 2.61:1 in any of the
    // thirteen. A state badge nobody can read is not reporting a state.
    // Scripted rather than string-matched, so the NEXT badge is caught too.
    // Those pills are the shared .badge now, so this file is where the pairing
    // lives; the two panel sheets stay under the guard so a local pill drawn
    // beside them cannot reintroduce the mistake.
    expect(css).toMatch(
      /\.badge \{[^}]*background: var\(--badge-bg\);[^}]*color: var\(--badge-fg\);/u);
    for (const name of ["CollaborationPanel"] as const) {
      const rules = declarationBlocks(component(name));
      const filled = rules.filter(rule => rule.body.includes("var(--badge-bg)"));
      for (const { selector, body } of filled) {
        expect(body, `${name} ${selector}`).toContain("color:var(--badge-fg)");
        expect(body, `${name} ${selector}`).not.toContain("var(--fg-dim)");
        // And on --radius-pill, not a 999px literal the Bare-CLI form axis
        // cannot reach — these are capsules like every other badge in the app.
        expect(body, `${name} ${selector}`).not.toContain("999px");
      }
      // The other half of the same mistake: a state variant that repaints one of
      // those pills keeps the fill it inherits, so --danger-fg landed on
      // --badge-bg — red on blue in all thirteen themes. Recolouring a badge
      // means bringing the surface the new colour is measured against.
      const badgeScopes = new Set(filled.flatMap(rule =>
        rule.selector.split(",").map(one => one.trim().split(/\s+/u)[0])));
      for (const { selector, body } of rules) {
        const recolours = body.match(/(?:^|[^-])color:\s*([^;]+);/u)?.[1];
        if (recolours === undefined || recolours === "var(--badge-fg)") continue;
        const scopes = selector.split(",").map(one => one.trim().split(/\s+/u));
        if (!scopes.some(parts => parts.length > 1 && badgeScopes.has(parts[0]))) continue;
        expect(body, `${name} ${selector} repaints a badge`).toMatch(/background:/u);
      }
    }
  });

  it("keeps neutral launcher rows legible at rest, on hover and when selected", () => {
    for (const { id, body } of allThemeBlocks) {
      for (const [foreground, background] of [
        ["fg", "surface-raised"], ["fg-dim", "surface-raised"],
        ["fg-strong", "row-hover"], ["fg-strong", "row-selected"],
      ]) {
        expect(contrastRatio(resolvedToken(body, foreground),
          surfaceHex(body, background, resolvedToken(body, "terminal-canvas"))),
          `${id} launcher ${foreground} on ${background}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("pins visible focus, minimum shared hitboxes, and container-aware pane layout", () => {
    expect(contrastRatio(token(lightBlock, "focus"), token(lightBlock, "editor-bg")))
      .toBeGreaterThanOrEqual(3);
    expect(contrastRatio(token(darkBlock, "focus"), token(darkBlock, "editor-bg")))
      .toBeGreaterThanOrEqual(3);
    expect(token(lightBlock, "control-h")).toBe("30px");
    expect(token(lightBlock, "titlebar-h")).toBe("44px");
    // The status bar carries buttons, so it answers to WCAG 2.5.8's 24px floor
    // like every other strip. It was 22, called a deliberate step tighter.
    expect(css.match(/\n\.statusbar \{[\s\S]*?\n\}/u)?.[0]).toContain("height: 24px;");
    expect(css.match(/\n\.statusbar-item \{[\s\S]*?\n\}/u)?.[0]).toContain("min-height: 24px;");
    // ONE ring, stated as two tokens. The global rule was 1px at +2px while
    // sixteen of the seventeen local rules drew 2px across three offsets, so
    // the app shipped a single indicator in four sizes. The width and offset
    // now come from here and the local rules read them; a rule that has to sit
    // inside its element negates the offset rather than inventing a number.
    expect(token(lightBlock, "focus-ring-w")).toBe("2px");
    expect(token(lightBlock, "focus-ring-offset")).toBe("2px");
    expect(css).toContain("outline: var(--focus-ring-w) solid var(--focus)");
    expect(css).toContain("outline-offset: var(--focus-ring-offset)");
    expect(css).not.toMatch(/outline:\s*\d+px solid var\(--focus\)/u);
    expect(css).not.toContain("box-shadow: 0 0 0 1px var(--focus-contrast)");
    expect(css).toContain("container: workspace / inline-size");
    expect(css).toContain("@container workspace (min-width: 1140px)");
    expect(css).toContain("@media (max-width: 840px)");
    expect(css).toContain(".workspace-toolbar-traffic");
    expect(css).not.toContain(".workspace-toolbar-new");
  });

  it("draws that one ring in the component sheets too, never a second size", () => {
    // The tokens are only a vocabulary until the rules that draw their own ring
    // read them. Every :focus rule in these sheets is checked, not a listed
    // few, so a new one cannot quietly reintroduce a fourth size.
    const sheets = [
      ...["AgentCanvas", "AgentFeed", "AgentTask", "Composer", "ContextValueDialog",
        "Conversation", "EffortBar", "FastModeToggle",
        "SessionTile", "SettingsPage", "Sidebar", "SubagentPanel"]
        .map(name => ({ name, source: component(name) })),
      { name: "terminal.css", source: terminalCss },
    ];
    for (const { name, source } of sheets) {
      const rings = declarationBlocks(source).filter(rule =>
        rule.selector.includes(":focus") && /outline:\s*[^;]*var\(--focus\)/u.test(rule.body));
      expect(rings.length, name).toBeGreaterThan(0);
      for (const { selector, body } of rings) {
        expect(body.match(/outline:\s*([^;]+);/u)?.[1], `${name} ${selector}`)
          .toBe("var(--focus-ring-w) solid var(--focus)");
        // Inset is a legitimate need — a clipped scroller, pills sitting
        // shoulder to shoulder — but it negates the shared offset rather than
        // picking a number of its own.
        expect(
          ["var(--focus-ring-offset)", "calc(-1 * var(--focus-ring-offset))"],
          `${name} ${selector}`,
        ).toContain(body.match(/outline-offset:\s*([^;]+);/u)?.[1]);
      }
    }
  });

  it("answers a press on every button primitive, and says so once", () => {
    // The sheet had three :active rules against a hundred-odd :hover ones, at
    // two different scales — so most controls went hover → click → nothing, and
    // the two that did answer disagreed about how far. One rule, one scale.
    const press = declarationBlocks(cssRules)
      .filter(rule => rule.selector.includes(":active") && rule.body.includes("transform"));
    expect(press).toHaveLength(1);
    for (const primitive of [".btn", ".btn-danger", ".workspace-toolbar-btn",
      ".workspace-icon-btn", ".sidebar-nav-button", ".surface-switch-tab",
      ".settings-nav-link", ".app-select-trigger",
      ".conv-iconbtn", ".conv-send", ".conv-anchor-step",
      ".composer-icon-btn", ".composer-submit", ".fastmode-toggle",
      ".terminal-launcher-profile", ".terminal-launcher-count",
      ".terminal-launcher-close", ".terminal-launcher-review-link",
      ".adversarial-review-button", ".terminal-pane-action",
      ".terminal-tab-close", ".terminal-center-action"]) {
      expect(press[0].selector, primitive).toContain(`${primitive}:active:not(:disabled)`);
    }
    expect(press[0].body).toContain("transform: scale(.98);");
    // Not guarded for reduced motion, and that is deliberate: the global block
    // collapses transition-duration so the scale lands instantly rather than
    // animating. It does not reset transform, and a press that leaves no trace
    // is the accessibility problem, not the fix.
    expect(css).toMatch(/prefers-reduced-motion: reduce[\s\S]*transition-duration: 0\.01ms !important/u);
    const reducedMotion = [...css.matchAll(
      /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/gu,
    )].map(([, body]) => body);
    expect(reducedMotion.length).toBeGreaterThan(0);
    for (const body of reducedMotion) expect(body).not.toMatch(/[^-]transform:\s*none/u);
  });

  it("keeps no rule for markup the app stopped rendering", () => {
    // Every one of these is either asserted absent from the markup by another
    // suite (workspace-ui, AgentTile, Sidebar) or referenced nowhere outside
    // this sheet. A selector that matches nothing is a trap: it reads as
    // evidence the element still exists. The prose explaining each deletion
    // stays — hence stripComments, or the comments would fail their own test.
    for (const gone of ["workspace-toolbar-ide", "workspace-toolbar-count",
      "workspace-toolbar-alert", "workspace-toolbar-label",
      "workspace-tile-identity", "workspace-tile-meta", "workspace-tile-status",
      "workspace-tile-separator", "workspace-tile-model", "banner-engine", "banner-error",
      "sidebar-repo-context", "conv-user", "usage-chip", "banner", "card",
      "badge-danger", "pp-link", "statusbar-count", "sidebar-section-title"]) {
      expect(cssRules, gone).not.toMatch(new RegExp(`\\.${gone}\\b`, "u"));
    }
    // One --material-blur, not two. Both said 0px, so the duplicate changed
    // nothing and hid that fact.
    expect(lightBlock.match(/--material-blur:/gu)).toHaveLength(1);
  });

  it("declares the state pill once, in two variants", () => {
    // Eight component rules drew their own capsule before this existed, which
    // is how one of them put --fg-dim on --badge-bg and another --danger-fg.
    // The pair is contrast-tested per theme by the badge assertions above; the
    // primitive is what makes that measurement reach the pills people see.
    const badge = css.match(/\n\.badge \{[\s\S]*?\n\}/u)?.[0] ?? "";
    expect(badge).toContain("border-radius: var(--radius-pill);");
    expect(badge).toContain("background: var(--badge-bg);");
    expect(badge).toContain("color: var(--badge-fg);");
    expect(css).toMatch(/\n\.badge-quiet \{[\s\S]*?box-shadow: inset 0 0 0 var\(--border-w\) var\(--border-strong\);/u);
    // And the form axis already squares it, so no CLI rule of its own.
    expect(css).toMatch(/:root\[data-chrome="cli"\][\s\S]*?--radius-pill: 0;/u);
  });

  it("keeps the Dev product chip above AA on its own canary fill", () => {
    expectTextContrast(token(lightBlock, "product-dev-fg"), token(lightBlock, "product-dev-bg"));
    expect(token(lightBlock, "product-dev-bg")).toBe("#e0b41c");
    expect(css).toContain(".product-dev-badge");
  });

  it("keeps a subagent cell a ROW, not a card, and its nesting quiet", () => {
    // THE SHAPE. This test used to lock the opposite: a filled, bordered,
    // 8px-radiused card with a 2px status rail and a second metadata line —
    // 52px plus margin per agent against a tool row's 28px, so three parallel
    // agents filled the pane and read as three boxes in a column of rows.
    // Those four properties are what made it a card, so their ABSENCE is the
    // assertion; the reasoning the old test carried (tokens only, states
    // distinguishable, nesting safe) is kept below in the new shape.
    const cell = css.match(/\n\.sub \{[\s\S]*?\n\}/u)?.[0] ?? "";
    expect(cell).not.toBe("");
    expect(cell).not.toContain("background:");
    expect(cell).not.toContain("border:");
    expect(cell).not.toContain("border-radius:");
    // The rail and the four status colours it took are gone with it. Status is
    // carried by the same StateGlyph vocabulary — pulsing dot, check, error,
    // stop — that every other row in the transcript uses, so a per-status rail
    // on this row and nothing like it on its neighbours was a second copy of
    // the status in a language nothing else spoke.
    expect(css).not.toContain("--sub-accent");
    expect(css).not.toContain(".sub::before");
    // GEOMETRY MATCHES .conv-row (Conversation.tsx): a fixed --row-h rather
    // than a min-height free to grow, and 6px padding rather than 8px, which
    // is what put the old cell 2px off the transcript's horizontal grid.
    const row = css.match(/\n\.sub-row \{[\s\S]*?\n\}/u)?.[0] ?? "";
    expect(row).toContain("height: var(--row-h);");
    expect(row).toContain("padding: 0 6px;");
    expect(row).not.toContain("min-height");
    // The 2px rail returns as the SELECTION marker, on one cell at a time,
    // which is what a rail is for. Both tokens are already measured against
    // --row-selected (over chrome-bg AND editor-bg, in every shipped theme) by
    // the hover/selected test above: --accent and --fg-muted are in
    // rowTextTokens, and --row-selected is in rowSurfaces.
    expect(css).toContain(".sub.is-selected .sub-row {");
    expect(css).toMatch(/\.sub\.is-selected \.sub-row \{[\s\S]*background: var\(--row-selected\);[\s\S]*box-shadow: inset 2px 0 0 var\(--accent\);/u);
    expect(css).toContain(".sub-state-run { color: var(--fg-muted); }");
    // The dead rules go with the second line they used to lay out. A selector
    // matching nothing is a trap for the next person to read this file.
    expect(css).not.toContain(".sub-meta");
    expect(css).not.toContain(".sub-activity");
    expect(css).not.toContain(".sub-model");
    // Nesting goes quiet with its parent: no border-top, no second ground, and
    // the guide line on --tree-guide, the token that exists for exactly this.
    const kids = css.match(/\n\.sub-kids \{[\s\S]*?\n\}/u)?.[0] ?? "";
    expect(kids).not.toContain("border-top");
    expect(kids).not.toContain("background:");
    expect(css).toMatch(/\.sub-kids::before\s*\{[\s\S]*background:\s*var\(--tree-guide\);/u);
  });

  it("adds the detail column as a modifier, never by editing the base grid", () => {
    // The base rule is asserted VERBATIM by workspace-ui.test.ts. A third track
    // that edited it would break the two-column shell for every window that
    // never opens a subagent, so the column arrives as .has-detail — and only
    // above the width where three columns fit.
    expect(css).toContain("grid-template-columns: var(--pane-left) minmax(0, 1fr);");
    expect(css).toContain("--pane-detail: 340px;");
    // NOT --pane-right: that name belongs to the retired fixed chat rail this
    // app deliberately removed, and workspace-ui.test.ts asserts its absence.
    expect(css).not.toContain("--pane-right");
    expect(css).toMatch(/@media \(min-width: 1081px\)[\s\S]*?\.app-grid\.has-detail \{\s*grid-template-columns: var\(--pane-left\) minmax\(0, 1fr\) var\(--pane-detail\);/u);
    // The sidebar-hidden shell needs its own two-track form, or hiding the
    // sidebar with the column open would leave a 288px hole where it was.
    expect(css).toContain(".app-grid.no-sidebar.has-detail {");
    // Below that width it becomes a non-modal side sheet. A backdrop would
    // make the surface pointer-modal while its complementary-region semantics
    // deliberately keep the visible transcript keyboard-operable.
    expect(css).toMatch(/@media \(max-width: 1080px\)[\s\S]*?\.app-grid\.has-detail \.detail-pane \{[\s\S]*position: fixed;/u);
    expect(css).not.toContain("detail-backdrop");
  });

  // Not "every Ghostty palette" any more: Grok Build and Nightshade come from a
  // CLI bundle and a screenshot, not from a .ghostty file. The contract was
  // never about where a palette came from — it is that every named block ships
  // a legible text ramp — so the name says that instead.
  it("ships every named palette and keeps its text tokens legible", () => {
    expect(namedThemeBlocks.map(block => block.id)).toEqual([
      // Porcelain and Porcelain Night are the Interface family's own blocks
      // (owner, 2026-09-26); the bare :root block is only the fallback.
      "light",
      "dark",
      // Sauteed Paper and Warm Editorial folded into Clay (the anthropic-*
      // blocks) and Nightshade into Carbon (grok-build), owner 2026-09-28.
      "anthropic-light",
      "anthropic-dark",
      "solarized-light",
      "solarized-dark",
      "catppuccin-mocha",
      "grok-build",
      "hermes",
      "hermes-light",
    ]);
    expect(css).toContain('--font-ui-serif:');
    expect(css).toContain(':root[data-font="serif"] .conv-text');
    expect(css).toContain(':root[data-font="serif"] .conv-markdown');
    expect(css).toContain("font-family: var(--font-ui-serif)");
    expect(css).not.toContain("--font-ui: var(--font-ui-serif)");

    for (const { id, body } of namedThemeBlocks) {
      const kind = token(body, "theme-kind");
      const editor = token(body, "editor-bg");
      const chrome = token(body, "chrome-bg");
      const selectedRaw = token(body, "row-selected");
      const selected = selectedRaw.length === 9
        ? compositeHex(selectedRaw, chrome) : selectedRaw;
      const dimSurface = kind === "dark" ? token(body, "surface-raised") : token(body, "row-hover");
      const placeholderSurface = kind === "dark" ? chrome : editor;
      expectTextContrast(token(body, "accent-text"), selected);
      expectTextContrast(token(body, "fg-selected-secondary"), selected);
      expectTextContrast(token(body, "fg-dim"), dimSurface);
      expectTextContrast(token(body, "input-placeholder"), placeholderSurface);
      expectTextContrast(token(body, "danger-fg"), token(body, "danger-bg"));
      expectTextContrast(token(body, "accent-fg"), token(body, "accent"));
      expect(contrastRatio(token(body, "focus"), editor)).toBeGreaterThanOrEqual(3);
      // Read from the registry rather than guessed from the id. The old rule
      // here was a suffix heuristic — "-dark", plus a hand-written exception for
      // catppuccin-mocha — which meant every dark-only palette that does not
      // spell "dark" in its name had to be added to it by hand or be asserted
      // to be LIGHT. THEME_KIND is what the main process actually reads to pick
      // the native window chrome, so a block whose --theme-kind disagrees with
      // it is the real defect, and this now catches that in either direction.
      expect(isResolvedTheme(id), `${id} is not a registered theme`).toBe(true);
      if (isResolvedTheme(id)) expect(kind).toBe(THEME_KIND[id]);
    }
  });

  it("holds every theme to AA on the hover and selected row states, not just the resting one", () => {
    for (const { id, body } of allThemeBlocks) {
      for (const surface of rowSurfaces(body)) {
        for (const name of rowTextTokens) {
          expect(
            contrastRatio(token(body, name), surface.hex),
            `--${name} on --${surface.name} in ${id}`,
          ).toBeGreaterThanOrEqual(4.5);
        }
        for (const name of rowGraphicTokens) {
          expect(
            contrastRatio(token(body, name), surface.hex),
            `--${name} on --${surface.name} in ${id}`,
          ).toBeGreaterThanOrEqual(3);
        }
      }
    }
  });

  it("keeps the destructive button's label legible on every state it paints", () => {
    // THE HOVER MUST NOT WALK THE BACKGROUND TOWARD THE TEXT. The label of a
    // danger button is --danger-fg on --danger-bg, so a hover that steps the
    // fill toward --danger-fg is spending the pair's own contrast on the
    // feedback. Measured over all thirteen theme blocks, the obvious 18% step
    // put NINE of them under 4.5:1 — 3.91 at worst (sauteed-paper-light) — and
    // even an 8% step left one theme at 4.53, three hundredths of headroom.
    // The three sheets ship a ring instead: the background is restated, never
    // mixed, and the inset shadow thickens from --border-w to 2px.
    const danger = [
      ["Conversation", ".btn.btn-danger"],
      ["ConfirmDialog", ".confirm-dialog-danger"],
      ["SteeringQueue", ".steering-queue-button-danger"],
    ] as const;
    for (const { id, body } of allThemeBlocks) {
      const fg = token(body, "danger-fg");
      const bg = token(body, "danger-bg");
      // Resting AND hover, because the ring hover leaves the background alone.
      expect(contrastRatio(fg, bg), `--danger-fg on --danger-bg in ${id}`)
        .toBeGreaterThanOrEqual(4.5);
      // The invariant behind the ruling, kept so the next author does not have
      // to rediscover it: mixing the fill toward the label ALWAYS costs the
      // pair contrast, in every theme, at every percentage. There is no safe
      // amount of it, only amounts that have not been measured yet.
      expect(contrastRatio(fg, mixHex(fg, bg, 0.18)), `18% danger mix in ${id}`)
        .toBeLessThan(contrastRatio(fg, bg));
    }
    for (const [name, selector] of danger) {
      const rules = declarationBlocks(component(name))
        .filter(rule => rule.selector.split(",").some(one => one.trim().startsWith(selector)));
      expect(rules.length, `${name} declares ${selector}`).toBeGreaterThanOrEqual(2);
      for (const { selector: where, body } of rules) {
        // No mix on any state of these rules: --danger-bg is the only surface
        // the label is ever measured against, so it must be the only one drawn.
        expect(body, `${name} ${where}`).not.toContain("color-mix");
        expect(body, `${name} ${where}`).toContain("var(--danger-bg)");
      }
      const hover = rules.find(rule => rule.selector.includes(":hover"));
      expect(hover?.body, `${name} ${selector} hover`).toContain("box-shadow:inset 0 0 0 2px var(--danger-fg)");
    }
  });

  it("keeps the pre-mount dark block token-for-token identical to the dark theme", () => {
    const preMount = css.match(
      /@media \(prefers-color-scheme: dark\) \{\s*:root:not\(\[data-theme\]\) \{([\s\S]*?)\n  \}/u,
    )?.[1] ?? "";
    const declared = (block: string): Map<string, string> =>
      new Map([...block.matchAll(/--([a-z0-9-]+):\s*([^;]+);/gu)]
        .map(([, name, value]) => [name, value.trim()] as const));
    const dark = declared(darkBlock);
    const early = declared(preMount);
    expect(early.size).toBeGreaterThan(0);
    expect([...early.keys()].sort()).toEqual([...dark.keys()].sort());
    for (const [name, value] of dark) expect(early.get(name), `--${name}`).toBe(value);
  });

  it("keeps diff text legible on its own tinted gutter", () => {
    for (const { id, body } of allThemeBlocks) {
      const editor = token(body, "editor-bg");
      for (const name of ["added", "removed"] as const) {
        expect(
          contrastRatio(
            token(body, `diff-${name}`),
            compositeHex(token(body, `diff-${name}-bg`), editor),
          ),
          `--diff-${name} on --diff-${name}-bg in ${id}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("declares one type ramp and one radius system, and obeys both", () => {
    expect(token(lightBlock, "text-meta")).toBe("11px");
    expect(token(lightBlock, "text-ui")).toBe("12px");
    expect(token(lightBlock, "text-body")).toBe("13px");
    expect(token(lightBlock, "text-label")).toBe("14px");
    expect(token(lightBlock, "text-nav")).toBe("16px");
    expect(token(lightBlock, "text-prose")).toBe("15px");
    expect(token(lightBlock, "text-h3")).toBe("15px");
    expect(token(lightBlock, "text-h2")).toBe("17px");
    expect(token(lightBlock, "text-h1")).toBe("19px");
    expect(token(lightBlock, "text-display")).toBe("22px");
    expect(token(lightBlock, "radius-pill")).toBe("999px");
    // Leading and tracking are ramps too. 22 distinct line-height values shipped
    // across 153 declarations, 16 of them px leading on token-sized text.
    expect(token(lightBlock, "lh-tight")).toBe("1.2");
    expect(token(lightBlock, "lh-ui")).toBe("1.35");
    expect(token(lightBlock, "lh-prose")).toBe("1.65");
    expect(token(lightBlock, "track-caps")).toBe(".04em");

    // The file that declares the ramps may not opt out of them. A raw px
    // font-size or a weight between 400 and 600 here is the ramp drifting at
    // its own source.
    const shorthand = /(?:^|[^-])font(?:-size)?:\s*[^;]*\b\d+px/gmu;
    expect(css.match(shorthand)).toBeNull();
    // WEIGHT IS A TOKEN NOW, NOT A LITERAL. The ramp was 400/600 while the
    // bundled Geist was two static instances and nothing between them could
    // render. The variable cut made the whole 100-900 axis real, so the ramp
    // moved down and closed up — 360/460/560, measured off Claude's own
    // desktop type — and the way to keep it honest changed with it: a literal
    // weight in a rule is now the drift, because it cannot be one of these
    // three. Any numeric font-weight in the file that declares the ramp is a
    // failure, whatever the number.
    expect(token(lightBlock, "wght-regular")).toBe("360");
    expect(token(lightBlock, "wght-medium")).toBe("460");
    expect(token(lightBlock, "wght-strong")).toBe("560");
    expect(token(lightBlock, "wght-prose")).toBe("430");
    const literalWeights = [...css.matchAll(/^[ \t]*font-weight:\s*(\d+)\s*;/gmu)]
      .map(([, w]) => w);
    expect(literalWeights).toEqual([]);
    // The browser's own em-sized defaults are on the ramp too, or <small>,
    // <strong> and an h4 in any pane are a size and a weight nothing declared.
    expect(css).toContain("small { font-size: var(--text-meta); }");
    expect(css).toContain("strong, b { font-weight: var(--wght-strong); }");
    expect(css).toContain("h4, h5, h6 { font-size: var(--text-body); }");
    // And the tokens have to be REACHED, or the ramp is three unread numbers.
    expect(css).toContain("font-weight: var(--wght-regular);");
    expect(css.match(/font-weight: var\(--wght-strong\)/gu)?.length ?? 0).toBeGreaterThan(5);
  });

  it("keeps every renderer stylesheet on that ramp, not on literal sizes", () => {
    // theme.css is only where the ramp is DECLARED. Almost all of the app's CSS
    // lives in the template strings inside App.tsx and components/*.tsx, plus
    // terminal.css, and a literal size there is the same drift as a literal in
    // theme.css — just further from the declaration and easier to miss. Every
    // spelling counts: `font-size: 11px`, a `font:` shorthand carrying a size
    // (the same declaration wearing a hat), and a relative unit, which is a
    // size off the ramp too — it just says so in em rather than px.
    const componentDir = new URL("./components/", import.meta.url);
    const sources = [
      { name: "App.tsx", source: readFileSync(new URL("./App.tsx", import.meta.url), "utf8") },
      { name: "terminal.css", source: terminalCss },
      ...readdirSync(componentDir).filter(name => /\.(tsx|css)$/.test(name)).sort()
        .map(name => ({ name, source: readFileSync(new URL(name, componentDir), "utf8") })),
    ];
    // A size may sit off the ramp when a ramp token would be WRONG rather than
    // merely different — but it says so here, by name, with its reason.
    const allowed = new Map<string, string>([
      ["Conversation.css:font-size:0.92em",
        "inline code scales with its parent's size"],
    ]);
    const literalSize =
      /(?:^|[^-\w])(font(?:-size)?:\s*[^;{}]*?\b\d*\.?\d+(?:px|r?em|%))/gu;
    const offenders: string[] = [];
    for (const { name, source } of sources) {
      for (const match of stripComments(source).matchAll(literalSize)) {
        const key = `${name}:${match[1].replace(/\s+/gu, " ").trim()}`;
        if (!allowed.has(key)) offenders.push(key);
      }
    }
    expect(offenders).toEqual([]);
    // And the allowlist may not outlive the site it excuses.
    for (const key of allowed.keys()) {
      const [name, declaration] = [key.slice(0, key.indexOf(":")), key.slice(key.indexOf(":") + 1)];
      expect(sources.find(entry => entry.name === name)?.source.replace(/\s+/gu, " "))
        .toContain(declaration);
    }
  });

  it("says disabled in colour, never in opacity", () => {
    // The token block states the rule — --fg-faint is decoration and disabled
    // controls — and opacity is the shortcut that quietly breaks it. It dims the
    // whole subtree, so the label, the ring, the glyph and the focus outline all
    // lose contrast together and none of them can be measured: a control at
    // .62 has no colour a contrast test can name. `color: var(--fg-faint)` says
    // the same thing at a ratio the palette guarantees.
    //
    // A `:not(:disabled)` rule counts too. Every one of them here carried an
    // `opacity: 1` that existed only to undo a dimming rule that is now gone —
    // an inert declaration reading as though the shortcut were still in use.
    const disabled = /:disabled|\[disabled\]|\[aria-disabled(?:="true")?\]|\.is-disabled/u;
    const offenders: string[] = [];
    for (const { name, source } of rendererSheets) {
      for (const rule of declarationBlocks(source)) {
        if (!disabled.test(rule.selector)) continue;
        for (const match of rule.body.matchAll(/(?:^|[^-\w])(opacity\s*:[^;{}\n]*)/gu)) {
          offenders.push(`${name}:${lineOf(source, rule.bodyIndex + match.index)} `
            + `${rule.selector.replace(/\s+/gu, " ")} { ${match[1].trim()} }`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("spends weight from the ramp, in the longhand and in the font shorthand", () => {
    // 600 and 700 are the OLD ramp — the two static Geist instances, the only
    // weights that could render before the variable cut. The shipped strong
    // tier is 560 now, so a literal 600 draws a weight nothing declared, one
    // visible step heavier than every heading beside it.
    //
    // The shorthand is the same declaration wearing a hat, and it is why this
    // guard exists at all: `font: 600 var(--text-meta)/1 var(--font-ui)` in
    // ProviderLogo survived the longhand sweep in Task 1 untouched. @font-face's
    // `font-weight: 100 900` is the variable AXIS RANGE, not a weight, and is
    // the one place a pair of numbers belongs.
    const offenders: string[] = [];
    for (const { name, source } of rendererSheets) {
      for (const rule of declarationBlocks(source)) {
        if (rule.selector.includes("@font-face")) continue;
        const at = (index: number): number => lineOf(source, rule.bodyIndex + index);
        for (const match of rule.body.matchAll(/font-weight:\s*([^;{}\n]+)/gu)) {
          if (/^(?:var\(--wght-[a-z]+\)|inherit)$/u.test(match[1].trim())) continue;
          offenders.push(`${name}:${at(match.index)} ${match[0].trim()}`);
        }
        // Everything before the size in a `font:` shorthand: a weight there is
        // a literal even though the size beside it reads as a token.
        for (const match of rule.body.matchAll(/(?:^|[^-\w])(font:\s*[^;{}\n]+)/gu)) {
          if (!/^font:\s*[^/]*?\b(?:\d{3}|bold|bolder)\b/u.test(match[1])) continue;
          offenders.push(`${name}:${at(match.index)} ${match[1].trim()}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("draws every corner from the radius system, not from a literal", () => {
    // Four graded tokens and a capsule, and the Bare-CLI form axis squares all
    // five at once by re-declaring them. A literal radius opts out of that
    // switch silently, which is the whole failure mode: .conv-steer and
    // .conv-send-queue are the same capsule side by side, and the one with the
    // literal stayed rounded in CLI chrome while its twin squared. theme.css
    // carried an ESCAPE LIST — a CLI rule zeroing the literals by name — which
    // is a second place to remember, and it had already gone stale.
    //
    // `0`, `50%` and `inherit` are not sizes: a square corner and a circle are
    // themselves at every density, and `inherit` takes whatever its parent
    // resolved. A calc() anchored to a radius token is on the system too — an
    // inner corner is the outer one minus its border, and it still moves when
    // the token does. --tile-screen-radius is that inner corner for a Porcelain
    // screen nested in its frame (the tile radius minus the frame inset).
    // Everything else names a token, or says here why it cannot.
    const onSystem = (value: string): boolean => value
      .replace(/calc\([^()]*var\(--(?:radius-[a-z]+|tile-radius|tile-screen-radius)\)[^()]*\)/gu, " token ")
      .replace(/var\(--(?:radius-[a-z]+|tile-radius|tile-screen-radius)\)/gu, " token ")
      .trim().split(/[\s/]+/u).filter(Boolean)
      .every(part => ["token", "0", "50%", "inherit"].includes(part));
    const allowed = new Map<string, string>([
      ["components/WorkingIndicator.css: .working-wave > span { border-radius: 999px }",
        "the owner-selected Wave is a status glyph with rounded bars in both chrome "
        + "styles, not a control whose corners follow the Bare CLI form axis"],
      ['theme.css: :root[data-chrome="cli"] .conv-turn-user { border-radius: 3px }',
        "the CLI form axis zeroes every radius token, so its own two drawn "
        + "corners cannot be one: a 3px tint band is the shape a TUI gives a "
        + "quoted line, and no token can be 3px while --radius-control is 0"],
      ['theme.css: :root[data-chrome="cli"] .conv-approval { border-radius: 3px }',
        "the same 3px, on the one card CLI still draws as a card"],
      ["theme.css: .product-dev-badge { border-radius: 0.28em }",
        "the Dev chip is drawn in em throughout — margin, padding, size and "
        + "vertical-align — so it scales with the wordmark it sits beside. A px "
        + "token would hold still while the mark around it grew"],
      // The Clean Stop/Send pair kept a literal 10px here while Send's corner
      // lived in a sheet this scan cannot reach. Both are capsules on
      // --radius-pill now, in one sheet, so the excuse is gone with the corner.
    ]);
    const offenders: string[] = [];
    const seen = new Set<string>();
    for (const { name, source } of rendererSheets) {
      for (const rule of declarationBlocks(source)) {
        for (const match of rule.body.matchAll(/border-radius:\s*([^;{}\n]+)/gu)) {
          const value = match[1].trim();
          const key = `${name}: ${rule.selector.replace(/\s+/gu, " ")} `
            + `{ border-radius: ${value} }`;
          seen.add(key);
          if (onSystem(value) || allowed.has(key)) continue;
          offenders.push(`${name}:${lineOf(source, rule.bodyIndex + match.index)} `
            + `${rule.selector.replace(/\s+/gu, " ")} { border-radius: ${value} }`);
        }
      }
    }
    expect(offenders).toEqual([]);
    // And an excuse may not outlive the corner it excuses.
    for (const key of allowed.keys()) expect(seen).toContain(key);
  });

  it("sets every line on one of the three leading tiers", () => {
    // 22 distinct line-height values across 153 declarations, with 1.3, 1.35,
    // 1.4, 1.45 and 1.5 all inside a single component — not five decisions,
    // five defaults nobody compared. The ramp says what each tier is for:
    // --lh-tight a row, a label, a pill, a heading that owns its own box;
    // --lh-ui pane and dialog body, the default for anything multi-line;
    // --lh-prose the transcript's reading measure.
    //
    // A unitless 1 stays: an icon box and a ticker glyph want the glyph's own
    // height and no leading at all. `inherit` and `normal` opt out by naming
    // someone else's decision rather than inventing a sixth number.
    //
    // px leading is the worst of the twenty-two, because it silently pins the
    // line to a size the rule no longer controls: a count pill centred its
    // digit with `line-height: 16px` in a 16px box, so the pill came apart the
    // moment --text-meta moved. Those are on the ramp with a grid now.
    //
    // The `font:` shorthand carries leading too, after the slash, and it is the
    // same hole the weight guard above closes: `font: 12px/1.42 x` sets a
    // twenty-third value that no line-height longhand anywhere would show.
    //
    // The two blocks the sheet exempts in prose (theme.css, "Leading and
    // tracking") keep their own: the Bare-CLI form axis matches the user's
    // terminal metrics rather than this ramp, and the Hermes skin's display
    // wordmarks are drawn type set at 0.95-1.08.
    const exemptBlock = /^:root\[data-chrome="cli"\]/u;
    const onRamp = /^(?:var\(--lh-(?:tight|ui|prose)\)|1|inherit|normal)$/u;
    const offenders: string[] = [];
    for (const { name, source } of rendererSheets) {
      for (const rule of declarationBlocks(source)) {
        if (rule.selector.split(",").every(one => exemptBlock.test(one.trim()))) continue;
        const at = (index: number): string =>
          `${name}:${lineOf(source, rule.bodyIndex + index)} `
          + `${rule.selector.replace(/\s+/gu, " ")}`;
        for (const match of rule.body.matchAll(/line-height:\s*([^;{}\n]+)/gu)) {
          const value = match[1].trim();
          if (onRamp.test(value)) continue;
          offenders.push(`${at(match.index)} { line-height: ${value} }`);
        }
        for (const match of rule.body.matchAll(
          /(?:^|[^-\w])(font:\s*[^;{}\n]*?\/\s*([^\s;{}]+))/gu)) {
          if (onRamp.test(match[2])) continue;
          offenders.push(`${at(match.index)} { ${match[1].trim()} }`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("tracks an uppercase label by one amount and nothing small at all", () => {
    // Two tiers, and they do opposite jobs. --track-caps is the ONE amount an
    // uppercase micro-label is opened up by; five different values were in use
    // for that same kind of label — .02, .03, .04, .06 and .13em, the last of
    // them three times the first, on section headings one pane apart.
    //
    // --track-h1 is the other job: an optical-size stand-in that TIGHTENS a
    // display size. Its own comment sets the floor — "below ~17px Geist's
    // default fit is already right, and tightening small dense UI text is how a
    // transcript becomes hard to scan" — so a monogram tile, a brand name and a
    // library title carrying -.01 to -.04em at 11-13px were spending the
    // display tier on exactly the text it excludes. Those declarations are
    // gone rather than retokenised: the correct amount there is none.
    //
    // `0`, `normal` and `inherit` stay — a nested hint turning OFF the caps
    // tracking it inherits is naming a decision, not inventing a value.
    const offenders: string[] = [];
    for (const { name, source } of rendererSheets) {
      for (const rule of declarationBlocks(source)) {
        for (const match of rule.body.matchAll(/letter-spacing:\s*([^;{}\n]+)/gu)) {
          const value = match[1].trim();
          if (/^(?:var\(--track-(?:caps|h1)\)|0|normal|inherit)$/u.test(value)) continue;
          offenders.push(`${name}:${lineOf(source, rule.bodyIndex + match.index)} `
            + `${rule.selector.replace(/\s+/gu, " ")} { letter-spacing: ${value} }`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("gives no fallback to a token :root already declares", () => {
    // A fallback is a claim that the name might be missing. For anything in the
    // :root block it cannot be — the guard above proves every one of them is
    // declared — so the second value is dead code that reads as doubt, and it
    // rots: `var(--ease-out, ease-out)` names a curve nothing would ever use
    // (the linear-ish CSS keyword, not the sheet's cubic-bezier), and
    // `var(--terminal-content-bg, var(--editor-bg))` restates a default the
    // token block sets one line from its declaration. Six sheets read
    // --terminal-content-bg bare; only one hedged.
    //
    // The hedge is RIGHT for a locally-scoped property: --working-spark and
    // --provider-accent are declared per [data-provider] in ProviderLogo, so
    // an element outside a provider subtree genuinely has no value for them.
    // Those keep their fallbacks, and this guard leaves them alone by only
    // looking at names :root declares.
    //
    // EVERY :root block, not the first: the optical --track-h1 tier is declared
    // in a second one beside the rule it explains (theme.css, "OPTICAL SIZING,
    // APPROXIMATED"), and a guard that reads only the token block at the top
    // would exempt it and anything else declared away from the others.
    const rootTokens = new Set([...css.matchAll(/:root \{([\s\S]*?)\n\}/gu)]
      .flatMap(([, block]) => [...block.matchAll(/--([a-z0-9-]+):/gu)]
        .map(([, name]) => name)));
    expect(rootTokens.has("track-h1")).toBe(true);
    const offenders: string[] = [];
    for (const { name, source } of rendererSheets) {
      for (const match of stripComments(source).matchAll(/var\(\s*--([a-z0-9-]+)\s*,/gu)) {
        if (!rootTokens.has(match[1])) continue;
        offenders.push(`${name}:${lineOf(source, match.index)} var(--${match[1]}, …)`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("declares a glyph for every codicon name the renderer emits", () => {
    // codicons.css says it itself: "Missing declarations render as an empty
    // 16px box, not as nothing — silent clutter." The fast-mode bolt, three
    // Settings→Updates buttons and the transcript's plan marks all shipped as
    // empty boxes because nothing checked the emitted names against the sheet.
    const componentDir = new URL("./components/", import.meta.url);
    const sources = [
      { name: "App.tsx", source: readFileSync(new URL("./App.tsx", import.meta.url), "utf8") },
      { name: "markdown.tsx", source: readFileSync(new URL("./markdown.tsx", import.meta.url), "utf8") },
      ...readdirSync(componentDir).filter(name => /\.(tsx|css)$/.test(name)).sort()
        .map(name => ({ name: `components/${name}`, source: readFileSync(new URL(name, componentDir), "utf8") })),
    ];
    const codicons = readFileSync(new URL("./codicons.css", import.meta.url), "utf8");
    const declared = new Set(
      [...codicons.matchAll(/\.codicon-([a-z0-9-]+)::before/gu)].map(([, name]) => name));
    const offenders: string[] = [];
    for (const { name, source } of sources) {
      const emitted = new Set<string>();
      // A class literal. `codicon-sm` and the modifiers are behaviour, not
      // glyphs; a fragment ending in `-` is a template building its name at
      // runtime (`codicon-chevron-${up|down}`) and has nothing to check.
      for (const [, glyph] of source.matchAll(/codicon-([a-z0-9-]*)/gu)) {
        if (glyph === "" || glyph === "sm" || glyph.startsWith("modifier") || glyph.endsWith("-")) continue;
        emitted.add(glyph);
      }
      // And the <Icon name=…> prop, whose value reaches the same class — as a
      // bare string or as the string literals inside a ternary.
      for (const [, literal, expression] of source.matchAll(/<Icon\s+name=(?:"([a-z0-9-]+)"|\{([^{}]*)\})/gu)) {
        if (literal !== undefined) emitted.add(literal);
        // The branches of a ternary are names; the string a condition compares
        // AGAINST ("completed", "error") is not, so drop those operands first —
        // on either side of the operator, since `"x" === status` reads the same
        // way round.
        const branches = (expression ?? "")
          .replace(/[!=]==?\s*"[a-z0-9-]+"/gu, "")
          .replace(/"[a-z0-9-]+"\s*[!=]==?/gu, "");
        for (const [, choice] of branches.matchAll(/"([a-z0-9-]+)"/gu)) emitted.add(choice);
      }
      for (const glyph of emitted) {
        if (!declared.has(glyph)) offenders.push(`${name}: codicon-${glyph}`);
      }
    }
    expect(offenders.sort()).toEqual([]);
  });

  it("reads every token it declares, and declares no name nothing reads", () => {
    // The other half of the guard below. Nineteen names were declared and read
    // by nothing — five of them "legacy aliases" whose own comment said to
    // delete each one as its last call site moved, and every call site had.
    // A token surface larger than the app that uses it is a design system
    // describing a version of itself that no longer exists.
    const componentDir = new URL("./components/", import.meta.url);
    const renderer = [
      css, terminalCss,
      readFileSync(new URL("./App.tsx", import.meta.url), "utf8"),
      readFileSync(new URL("./markdown.tsx", import.meta.url), "utf8"),
      ...readdirSync(componentDir).filter(name => /\.(tsx|css)$/.test(name))
        .map(name => readFileSync(new URL(name, componentDir), "utf8")),
    ].join("\n");
    // Two exceptions, each with a reader that is not a var():
    //   --theme-kind is read by the main process through THEME_KIND, and the
    //     per-palette loop above cross-checks the two.
    //   --diff-added-bg / --diff-removed-bg are the missing half of the diff
    //     gutter, held to AA per theme by the contrast test below so the tint
    //     is measured before anything paints it.
    //   --material-widget lost its last reader when the terminal's floating
    //     toolbar left (owner, 2026-11-15). Its .is-floating override is a
    //     marked line of the "Floating menus and dialogs" settled look, so it
    //     goes in a change that may touch that look, not in passing.
    //   --terminal-text-weight is the weight of the terminal's own face
    //     (MonoLisa Bold, the owner's Rex setting), read by TerminalView's
    //     terminalFont through getComputedStyle for xterm's fontWeight option;
    //     TerminalPrimitives.test.ts pins that read. It is a face, not a step
    //     of the chrome's --wght ramp.
    const exempt = new Set([
      "theme-kind", "diff-added-bg", "diff-removed-bg", "material-widget", "terminal-text-weight",
    ]);
    const declared = [...new Set([...lightBlock.matchAll(/--([a-z0-9-]+):/gu)]
      .map(([, name]) => name))];
    const unread = declared.filter(name =>
      !exempt.has(name) && !new RegExp(`var\\(\\s*--${name}\\s*[,)]`, "u").test(renderer));
    expect(unread).toEqual([]);
  });

  it("declares every custom property the renderer reads without a fallback", () => {
    // An undeclared var() is not an error anywhere: the declaration is dropped
    // at computed-value time and the property silently keeps its inherited or
    // initial value. That is how --text-title (shipped, fixed), --radius-panel,
    // --surface-hover and --warning-fg each reached users as a panel with no
    // radius or a hover that never painted. A var() WITH a fallback is a
    // deliberate optional hook and is not swept here; a bare one is a promise
    // that the name exists.
    const componentDir = new URL("./components/", import.meta.url);
    const sources = [
      { name: "App.tsx", source: readFileSync(new URL("./App.tsx", import.meta.url), "utf8") },
      { name: "markdown.tsx", source: readFileSync(new URL("./markdown.tsx", import.meta.url), "utf8") },
      { name: "theme.css", source: css },
      { name: "terminal.css", source: terminalCss },
      ...readdirSync(componentDir).filter(name => /\.(tsx|css)$/.test(name)).sort()
        .map(name => ({ name: `components/${name}`, source: readFileSync(new URL(name, componentDir), "utf8") })),
    ];
    // `--name:` in a rule body, and `"--name":` in an inline style object.
    const declaredIn = (source: string): Set<string> =>
      new Set([...source.matchAll(/--([a-z0-9-]+)['"]?\s*:/gu)].map(([, name]) => name));
    const themeTokens = declaredIn(css);
    // Locally scoped custom properties whose declaration this scan cannot see
    // from the file that reads them: --provider-accent is declared per
    // [data-provider] in ProviderLogo.tsx and read by terminal.css, and
    // --working-spark beside it is only ever read WITH a fallback. The other
    // three locals (--conv-skel, --effort-color, --pp-depth) are discovered by
    // the same-file rule above and deliberately not listed here. --project-h
    // and --project-h2 are declared per .project-hue-N in ProjectHue.css and
    // read by the sheets of what sits inside a hue class (the New thread card).
    const localScoped = new Set(["provider-accent", "working-spark", "project-h", "project-h2"]);
    const offenders: string[] = [];
    for (const { name, source } of sources) {
      const local = declaredIn(source);
      for (const [, token] of stripComments(source).matchAll(/var\(\s*--([a-z0-9-]+)\s*\)/gu)) {
        if (themeTokens.has(token) || local.has(token) || localScoped.has(token)) continue;
        offenders.push(`${name}: --${token}`);
      }
    }
    expect([...new Set(offenders)]).toEqual([]);
  });

  it("uses one bounded motion vocabulary with an explicit reduced-motion path", () => {
    expect(token(lightBlock, "motion-fast")).toBe("100ms");
    expect(token(lightBlock, "motion-base")).toBe("180ms");
    expect(token(lightBlock, "motion-slow")).toBe("260ms");
    expect(token(lightBlock, "ease-out")).toContain("cubic-bezier");
    expect(css).toContain("@keyframes workspace-surface-in");
    expect(css).toContain("@keyframes detail-pane-in");
    expect(css).toMatch(/prefers-reduced-motion: reduce[\s\S]*animation-duration: 0\.01ms !important/u);
    expect(css).toMatch(/prefers-reduced-motion: reduce[\s\S]*\.app-grid, \.app-grid > \.sidebar, \.app-grid > \.workspace-shell \{ transition: none; \}/u);
  });

  it("draws nothing for an idle dot, so the working dot is found by absence around it", () => {
    // Idle rows used to carry a grey disc and the working dot needed a halo
    // to be spotted among them (owner, 2026-09-12). The column stays.
    expect(cssRules).toMatch(/\.dot-idle::before\s*\{ background: transparent; \}/u);
    expect(cssRules).not.toMatch(/dot-halo/u);
  });

  it("defines the shared visually-hidden rule a spoken-only notice relies on", () => {
    // The Fast toggle's unconfirmed notice used .sr-only while no sheet defined
    // it, so the sentence was drawn inside the 22px pane-header button.
    const rule = cssRules.match(/(?:^|\n)\.sr-only \{[^}]*\}/u)?.[0] ?? "";
    expect(rule).toContain("position: absolute;");
    expect(rule).toContain("clip: rect(0, 0, 0, 0);");
    expect(rule).toContain("overflow: hidden;");
    expect(component("TerminalFastModeToggle")).toContain('<span className="sr-only" role="status">');
  });

  it("keeps the version-control mark off the attention hue", () => {
    // The file tree's "modified" badge and its collapsed-folder dot read this
    // token. It must exist (an undeclared var() paints nothing) and it must not
    // be routed to the one colour that means "an agent is blocked on you".
    expect(token(lightBlock, "vcs-modified")).toBe("var(--fg-muted)");
    expect(token(lightBlock, "vcs-modified")).not.toContain("status-waiting");
  });

  it("keeps the usage meter's amber for near-limit and off a stale reading", () => {
    // "You are about to be blocked" is a genuine summons and keeps the hue;
    // "this number was read a while ago" is not, and says so in words instead.
    expect(css).toContain(".usage-bar-fill.is-high { background: var(--status-waiting); }");
    expect(css).toContain(
      ".usage-read { flex: none; color: var(--fg-dim); font-size: var(--text-meta); }");
    expect(css).not.toContain(".usage-read.is-stale");
  });

  it("spends the attention hue on exactly five rules in this stylesheet", () => {
    // Amber says ONE thing: an agent is blocked on you, or a limit is about to
    // block you. A broken provider is a fault (--danger-fg), a dismissible
    // worktree notice is passive information (--fg-dim), and a toolbar rule
    // nothing emits is not a state at all.
    const owners = [...css.matchAll(/\n([^\n{}]+)\{([^{}]*)\}/gu)]
      .filter(([, , body]) => body.includes("var(--status-waiting)"))
      .map(([, selector]) => selector.trim());
    expect(owners).toEqual([
      ".dot-waiting::before",
      ".statusbar-warning",            // "Rate limited" — the wall itself
      // A waiting Threads pane's head, read once for its still amber line and
      // its "Needs you" pill (owner, 2026-10-07; it replaced the head's amber
      // dot). Same sentence as .dot-waiting::before, said where the blocked
      // agent is: a waiting pane IS blocked on you.
      '.workspace-agent[data-status="waiting"] .workspace-tile-head',
      ".usage-bar-fill.is-high",
      // The sidebar footer's "N providers need setup" (owner, 2026-11-15:
      // amber, one line in the Settings row) rides on the toggle that brings
      // the hidden sidebar back. A provider that must be set up first is
      // blocked on you.
      ".sidebar-toggle-setup-dot",
    ].sort((a, b) => css.indexOf(a) - css.indexOf(b)));
    // The provider fault moved under Settings in the sidebar footer (2026-09-28).
    expect(css).not.toContain(".statusbar-alert");
    expect(css).toContain(".notice-glyph { flex: none; color: var(--fg-dim); }");
    // The dead rule is gone; only the comment explaining why still names it.
    expect(css).not.toMatch(/\.workspace-toolbar-warning\s*\{/u);
  });

  it("spends the access-mode tokens on a rule, in every theme", () => {
    // A token every theme declares and no selector reads is invisible work, and
    // a class the components emit with no rule behind it is an inert attribute.
    // Both halves are asserted here because each one alone looks finished.
    for (const tone of ["plan", "accept", "danger", "auto"] as const) {
      expect(css).toContain(`.access-tone-${tone} .app-select-value { color: var(--mode-${tone}); }`);
    }
    // The class lands on the picker ROOT and the rule targets the value, which
    // is what puts it at two classes: .app-select-value declares --fg itself, so
    // a one-class rule would lose to the control's own resting state.
    expect(css).not.toContain(".app-select-value.access-tone-");
    // Every block declares all four, or switching themes would leave the pill
    // painting whatever the previous palette said. The pre-mount fallback is
    // held to the same list by the token-for-token test above.
    for (const { id, body } of allThemeBlocks) {
      for (const tone of ["plan", "accept", "danger", "auto"] as const) {
        expect(() => token(body, `mode-${tone}`), `${id} --mode-${tone}`).not.toThrow();
      }
    }
  });

  it("uses solid materials and palette-only theme overrides", () => {
    expect(token(lightBlock, "material-blur")).toBe("0px");
    expect(token(lightBlock, "material-saturate")).toBe("100%");
    const fontFamilies = new Set(["--font-ui", "--font-ui-sans", "--font-ui-serif", "--font-mono", "--font-terminal"]);
    for (const { id, body } of namedThemeBlocks) {
      for (const [, name, value] of stripComments(body).matchAll(/(--[\w-]+|color-scheme):\s*([^;]+);/gu)) {
        if (name === "color-scheme" || name === "--theme-kind" || fontFamilies.has(name)) continue;
        // A palette may put dark screens in light chrome (Porcelain), so the
        // screen's colour scheme is a keyword beside --theme-kind, not a colour.
        if (name === "--terminal-color-scheme") {
          expect(value.trim(), `${id} ${name}`).toMatch(/^(?:light|dark|normal)$/u);
          continue;
        }
        expect(resolvedValue(body, value.trim()), `${id} ${name} must be a palette color`)
          .toMatch(/^#[0-9a-f]{6}(?:[0-9a-f]{2})?$/iu);
      }
    }
    // A color theme cannot introduce separate component layouts or decorations.
    for (const sheet of rendererSheets) {
      for (const rule of declarationBlocks(sheet.source)) {
        if (!rule.selector.includes("[data-theme=")) continue;
        for (const part of rule.selector.split(",")) {
          expect(part.trim(), sheet.name).toMatch(/^:root\[data-theme="[^"]+"\]$/u);
        }
      }
    }
  });

  it("keeps terminal body, idle and focused header tiers distinct in every palette", () => {
    for (const { id, body } of allThemeBlocks) {
      const surfaces = ["terminal-content-bg", "terminal-header-bg", "terminal-header-bg-focused"];
      expect(new Set(surfaces.map(name => token(body, name))).size, id).toBe(surfaces.length);
      // The screen carries its own text colour (--terminal-fg, Porcelain's dark
      // screens in light chrome); the frame and panel are read in --fg.
      const screenFg = body.includes("--terminal-fg:") ? resolvedToken(body, "terminal-fg") : token(body, "fg");
      expect(contrastRatio(screenFg, token(body, "terminal-content-bg")), `${id} text on terminal-content-bg`)
        .toBeGreaterThanOrEqual(4.5);
      for (const name of ["terminal-header-bg", "terminal-header-bg-focused", "workbench-panel"]) {
        expect(contrastRatio(token(body, "fg"), token(body, name)), `${id} text on ${name}`).toBeGreaterThanOrEqual(4.5);
      }
      const edge = parseHexColor(token(body, "terminal-tile-edge"));
      expect(edge.r, id).toBe(edge.g);
      expect(edge.g, id).toBe(edge.b);
    }
  });

  it("frosts by elevation, through one class rather than a name list", () => {
    // The old list was thirteen selector names that had to be edited by hand
    // whenever a popover was added — and had not been, so seven of twenty-five
    // floating surfaces were glass and the rest were flat cards beside them.
    const frosted = declarationBlocks(cssRules).filter(rule =>
      rule.body.includes("saturate(var(--material-saturate))"));
    expect(frosted).toHaveLength(1);
    expect(frosted[0].selector).toBe(".is-floating");

    // And the class has to REACH every root, not merely appear somewhere in the
    // file. A file-granular version of this check passed Sidebar.tsx because one
    // of its two menus carried the class — which is exactly the shape of the bug
    // it is supposed to catch, one root of a pair left flat.
    //
    // So: for every rule that draws the floating shadow, take the class the rule
    // lands on, then require that EVERY class list in the markup naming that
    // class also names is-floating. A rule with no emitter at all passes without
    // saying anything, which is honest — a selector nobody renders frosts
    // nothing, and dead CSS is a different test's job.
    const componentDir = new URL("./components/", import.meta.url);
    const markup = [
      { name: "App.tsx", source: readFileSync(new URL("./App.tsx", import.meta.url), "utf8") },
      { name: "markdown.tsx", source: readFileSync(new URL("./markdown.tsx", import.meta.url), "utf8") },
      ...readdirSync(componentDir).filter(name => /\.(tsx|css)$/.test(name)).sort()
        .map(name => ({ name: `components/${name}`, source: readFileSync(new URL(name, componentDir), "utf8") })),
    ];
    /** Every string or template literal in a source that reads as a class list:
        interpolations blanked, and every remaining word a bare class name. That
        shape is what excludes prose ("Show sidebar"), ids (`${uid}-row-menu`)
        and the CSS template blocks, which always carry a brace or a colon. It
        does NOT require the literal to sit on a className attribute, because
        AppSelect assembles its list in a helper. */
    const classLists = (source: string): string[][] =>
      [...source.matchAll(/"([^"\n]*)"|`([^`]*)`/gu)]
        .map(([, quoted, templated]) => (quoted ?? templated ?? "")
          .replace(/\$\{[^}]*\}/gu, " ").split(/\s+/u).filter(Boolean))
        .filter(words => words.length > 0 && words.every(word => /^[a-z][a-z0-9-]*$/u.test(word)));
    /** The class a rule lands on: the last compound of a selector, when that
        compound is one bare class. `.app-grid.has-detail .detail-pane` is the
        detail pane; `.effort-popover.is-maximum .effort-thumb` is the thumb. */
    const landsOn = (selector: string): string[] =>
      selector.split(",")
        .map(one => one.trim().split(/\s+/u).at(-1) ?? "")
        .flatMap(compound => /^\.[a-z][a-z0-9-]*$/u.test(compound) ? [compound.slice(1)] : []);
    // These roots draw the floating shadow and are deliberately not frosted:
    const exempt = new Map<string, string>([
      ["conv-jump-latest", "a floating BUTTON, not a surface: its fill is the "
        + "opaque --surface-raised (Conversation.tsx:1242-1247), so a blur "
        + "behind it composites against nothing and changes no pixel"],
      ["detail-pane", "the subagent column. It is a pushed third track above "
        + "1080px and a fixed sheet below it (theme.css:2809); it was never on "
        + "the frost list and is not a popover"],
      ["effort-thumb", "the 24px knob on the effort slider. It takes the shadow "
        + "as a drawn lift on a control, not as elevation over content"],
      ["agent-overview", "the agent overview column. Like the detail pane it is "
        + "a pushed track beside the panes above 1080px and a side sheet over "
        + "them below it (terminal.css), not a popover; its fill is the opaque "
        + "pane background, so a blur behind it changes no pixel"],
      ["corner-notice", "an error notice in the main area's corner (owner, "
        + "2026-11-15). Its fill is the opaque --surface, so a blur behind it "
        + "changes no pixel and would only add a backdrop pass over the "
        + "terminals it sits on"],
      ["corner-notice-more", "the corner stack's \"N more\" pill: an opaque "
        + "--surface button, like conv-jump-latest"],
      ["output-preview", "the output preview is an opaque workspace column "
        + "on wide windows and a side sheet on narrow windows, like the agent "
        + "overview; it is not a floating menu or dialog"],
    ]);
    const unreached: string[] = [];
    for (const sheet of [...markup,
      { name: "theme.css", source: css }, { name: "terminal.css", source: terminalCss }]) {
      // A stylesheet's rules are emitted by markup somewhere else in the
      // renderer; a component's own rules are emitted by that component.
      const emitters = sheet.name.endsWith(".css")
        ? markup.flatMap(file => classLists(file.source))
        : classLists(sheet.source);
      for (const rule of declarationBlocks(sheet.source)) {
        if (!rule.body.includes("var(--overlay-shadow)")) continue;
        for (const name of landsOn(rule.selector)) {
          if (exempt.has(name)) continue;
          for (const words of emitters) {
            if (words.includes(name) && !words.includes("is-floating")) {
              unreached.push(`${sheet.name}: .${name} rendered as "${words.join(" ")}"`);
            }
          }
        }
      }
    }
    expect([...new Set(unreached)].sort()).toEqual([]);
  });

  it("retains the Hermes navy/cream and ice/Klein palettes", () => {
    const hermes = namedThemeBlocks.find(block => block.id === "hermes");
    const light = namedThemeBlocks.find(block => block.id === "hermes-light");
    expect(hermes, "missing [data-theme=hermes] palette").toBeDefined();
    expect(light, "missing [data-theme=hermes-light] palette").toBeDefined();
    if (hermes === undefined || light === undefined) return;
    expect(token(hermes.body, "chrome-bg")).toBe("#12276b");
    expect(token(hermes.body, "editor-bg")).toBe("#142762");
    expect(token(hermes.body, "surface")).toBe("#16244e");
    expect(token(hermes.body, "fg")).toBe("#eddcc8");
    // Hermes carries its own gold accent (owner, 2026-09-12): the cream one read
    // muddy on navy.
    expect(token(hermes.body, "accent")).toBe("#e9c46a");
    expect(token(hermes.body, "theme-kind")).toBe("dark");
    expect(token(light.body, "chrome-bg")).toBe("#f4f7fe");
    expect(token(light.body, "editor-bg")).toBe("#f8fafd");
    expect(token(light.body, "accent")).toBe("#2451f2");
    expect(token(light.body, "theme-kind")).toBe("light");
    expect(css).not.toContain("hermes-watermark");
    expect(css).not.toContain("--text-wordmark");

  });
});
