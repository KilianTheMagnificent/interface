import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { contrastRatio } from "./contrast";
import {
  DEFAULT_APP_PREFERENCES,
  UI_CHROME_PREFERENCES,
  isUiChromePreference,
  normalizeAppPreferences,
  parseAppPreferences,
  type AppPreferences,
} from "./settings";
import {
  applyAppearancePreferences, applySettingsChromeLock, applyUiChrome,
  initializeAppPreferences,
} from "./appearance";
import { SettingsPage, type SettingsPageProps } from "./components/SettingsPage";

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");

/* ---- a small cascade for custom properties ------------------------------

   These assertions have to be about VALUES, not about the stylesheet's text: a
   test that greps theme.css for a string the same commit added proves only that
   the string was typed. So the sheet is parsed into rules, the rules that match
   a root element in a given (theme, chrome) state are cascaded by specificity
   and source order, and every var() is resolved. What comes out is the token
   map a browser would compute, and that is what gets asserted.

   At-rule blocks are skipped wholesale, which models exactly the state the app
   ships in: the renderer writes an explicit data-theme (so the
   prefers-color-scheme fallback never applies) and no reduced-transparency or
   reduced-motion preference is set. The one exception is `@layer`: the token
   system sits in the interface-palette layer (so a theme file can override
   it), and a layered rule loses to every unlayered one, whatever its
   specificity. */

interface CssRule {
  selector: string;
  /** Source order among top-level rules; the tiebreak when specificity ties. */
  index: number;
  declarations: ReadonlyMap<string, string>;
  /** Declared inside an `@layer` block. */
  layered: boolean;
}

function parseDeclarations(body: string): Map<string, string> {
  const declarations = new Map<string, string>();
  const add = (text: string): void => {
    const colon = text.indexOf(":");
    if (colon < 0) return;
    const name = text.slice(0, colon).trim();
    const value = text.slice(colon + 1).trim();
    if (name === "" || value === "") return;
    declarations.set(name, value);
  };
  let depth = 0;
  let start = 0;
  for (let at = 0; at < body.length; at += 1) {
    const character = body[at];
    if (character === "(") depth += 1;
    else if (character === ")") depth -= 1;
    else if (character === ";" && depth === 0) {
      add(body.slice(start, at));
      start = at + 1;
    }
  }
  add(body.slice(start));
  return declarations;
}

function topLevelRules(source: string, rules: CssRule[] = [], layered = false): CssRule[] {
  const stripped = source.replace(/\/\*[\s\S]*?\*\//gu, "");
  let cursor = 0;
  while (cursor < stripped.length) {
    const open = stripped.indexOf("{", cursor);
    if (open < 0) break;
    const selector = stripped.slice(cursor, open).trim();
    let depth = 1;
    let scan = open + 1;
    while (scan < stripped.length && depth > 0) {
      const character = stripped[scan];
      if (character === "{") depth += 1;
      else if (character === "}") depth -= 1;
      scan += 1;
    }
    if (selector.startsWith("@layer")) {
      topLevelRules(stripped.slice(open + 1, scan - 1), rules, true);
    } else if (!selector.startsWith("@")) {
      rules.push({
        selector,
        index: rules.length,
        declarations: parseDeclarations(stripped.slice(open + 1, scan - 1)),
        layered,
      });
    }
    cursor = scan;
  }
  return rules;
}

const RULES = topLevelRules(css);

/** A selector list's parts, split on top-level commas only: `:not(a, b)` stays whole. */
function selectorParts(selector: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  for (let at = 0; at < selector.length; at += 1) {
    const character = selector[at];
    if (character === "(") depth += 1;
    else if (character === ")") depth -= 1;
    else if (character === "," && depth === 0) {
      parts.push(selector.slice(start, at));
      start = at + 1;
    }
  }
  parts.push(selector.slice(start));
  return parts.map(part => part.replace(/\s+/gu, " ").trim());
}

/**
 * Every declaration the sheet gives one selector, whether the selector is a
 * rule's whole prelude or one part of a grouped list, merged in source order so
 * a later rule wins as it does in the cascade. `index` is the last contributing
 * rule's; undefined when no rule names the selector.
 */
function ruleFor(selector: string): CssRule | undefined {
  const matching = RULES.filter(rule => selectorParts(rule.selector).includes(selector));
  const last = matching.at(-1);
  if (last === undefined) return undefined;
  return {
    selector,
    index: last.index,
    declarations: new Map(matching.flatMap(rule => [...rule.declarations])),
    layered: last.layered,
  };
}

const ROOT_ATTRIBUTE = /^:root\[data-(theme|chrome)="([^"]+)"\]$/u;

/** Does one comma-separated part select the root element in this state? */
function partMatchesRoot(part: string, theme: string | null, chrome: string | null): boolean {
  if (part === ":root") return true;
  const attribute = ROOT_ATTRIBUTE.exec(part);
  if (attribute === null) return false;
  return attribute[1] === "theme" ? theme === attribute[2] : chrome === attribute[2];
}

/** `:root` is one pseudo-class; `:root[attr="v"]` adds one attribute selector. */
const rootSpecificity = (part: string): number => (part === ":root" ? 1 : 2);

function cascadeRoot(theme: string | null, chrome: string | null): Map<string, string> {
  const winners: {
    layered: boolean;
    specificity: number;
    index: number;
    declarations: ReadonlyMap<string, string>;
  }[] = [];
  for (const rule of RULES) {
    for (const part of rule.selector.split(",")) {
      const trimmed = part.trim();
      if (!partMatchesRoot(trimmed, theme, chrome)) continue;
      winners.push({
        layered: rule.layered,
        specificity: rootSpecificity(trimmed),
        index: rule.index,
        declarations: rule.declarations,
      });
      break;
    }
  }
  winners.sort((left, right) => Number(right.layered) - Number(left.layered)
    || left.specificity - right.specificity || left.index - right.index);
  const declared = new Map<string, string>();
  for (const winner of winners) {
    for (const [name, value] of winner.declarations) declared.set(name, value);
  }
  return declared;
}

function resolveValue(
  declared: ReadonlyMap<string, string>,
  value: string,
  seen: ReadonlySet<string>,
): string {
  return value.replace(
    /var\(\s*(--[\w-]+)\s*(?:,([^()]*))?\)/gu,
    (_match: string, name: string, fallback?: string) => {
      if (seen.has(name)) return fallback?.trim() ?? "";
      const raw = declared.get(name);
      if (raw === undefined) return fallback?.trim() ?? "";
      return resolveValue(declared, raw, new Set([...seen, name]));
    },
  );
}

/** Every custom property a browser would compute on :root in this state. */
function computedTokens(theme: string | null, chrome: string | null): Map<string, string> {
  const declared = cascadeRoot(theme, chrome);
  const computed = new Map<string, string>();
  for (const [name, value] of declared) {
    if (!name.startsWith("--")) continue;
    computed.set(name, resolveValue(declared, value, new Set([name])));
  }
  return computed;
}

/** Read the shipped palette list off the sheet; `null` is the bare `:root` light block. */
function declaredThemes(): string[] {
  const names = new Set<string>();
  for (const rule of RULES) {
    for (const part of rule.selector.split(",")) {
      const match = ROOT_ATTRIBUTE.exec(part.trim());
      if (match !== null && match[1] === "theme") names.add(match[2]);
    }
  }
  return [...names];
}

const PALETTES: readonly (string | null)[] = [null, ...declaredThemes()];

const paletteName = (theme: string | null): string => theme ?? ":root (light)";

function differingTokens(before: Map<string, string>, after: Map<string, string>): string[] {
  const names = new Set([...before.keys(), ...after.keys()]);
  return [...names].filter(name => before.get(name) !== after.get(name)).sort();
}

/** The form axis, and nothing else, may move between refined and CLI. */
const FORM_TOKENS: ReadonlySet<string> = new Set([
  "--font-ui",
  "--radius-control", "--radius-container",
  "--radius-overlay", "--radius-composer", "--radius-pill",
  "--overlay-shadow", "--composer-lift",
  "--material-blur", "--material-saturate", "--material-chrome",
  "--material-surface", "--material-widget",
  "--dropdown-bg", "--widget-bg", "--menu-bg",
  "--floating-opacity", "--floating-blur", "--floating-saturate",
  "--floating-dropdown-bg", "--floating-widget-bg", "--floating-menu-bg",
  // Density: a terminal console is tighter than the shipped harness.
  "--pane-left", "--titlebar-h", "--row-h", "--section-header-h",
  "--control-h", "--strip-h",
]);

const RADIUS_TOKENS = [
  "--radius-control", "--radius-container",
  "--radius-overlay", "--radius-composer", "--radius-pill", "--radius-none",
] as const;

const settingsProps = (overrides: Partial<SettingsPageProps> = {}): SettingsPageProps => ({
  appearance: { preference: "system", resolved: "dark" },
  onAppearanceChange: vi.fn(),
  preferences: { ...DEFAULT_APP_PREFERENCES },
  onPreferencesChange: vi.fn(),
  engineHealth: [],
  onRefreshProviders: vi.fn(),
  customizationApi: {
    getDefaultDirectory: vi.fn(async () => "/work"),
    listCustomization: vi.fn(async (root: string) => ({ root, entries: [], truncated: false })),
  },
  onClose: vi.fn(),
  ...overrides,
});

describe("CLI chrome is a form axis over every palette", () => {
  it("covers the root fallback, all ten shipped palettes and both theme-file palettes", () => {
    // null is the bare :root fallback; Porcelain and Porcelain Night are the
    // named light and dark blocks (owner, 2026-09-26). custom-dark and
    // custom-light are what a theme file paints over (2026-10-02).
    expect(PALETTES).toHaveLength(13);
    expect(PALETTES).toContain("custom-dark");
    expect(PALETTES).toContain("custom-light");
    expect(PALETTES).toContain("light");
    expect(PALETTES).toContain("grok-build");
    expect(PALETTES).not.toContain("nightshade");
    expect(PALETTES).toContain("hermes");
    expect(PALETTES).toContain("hermes-light");
    expect(PALETTES).toContain("catppuccin-mocha");
    expect(PALETTES).toContain("anthropic-light");
    expect(PALETTES).toContain("anthropic-dark");
  });

  it("flattens shape, depth and glass in every palette without moving a colour", () => {
    for (const theme of PALETTES) {
      const refined = computedTokens(theme, "refined");
      const cli = computedTokens(theme, "cli");

      // Nothing outside the form vocabulary may move. Every text, accent,
      // status, mode and border token is outside it, so this one assertion is
      // what keeps theme-accessibility.test.ts's measured matrix intact.
      for (const name of differingTokens(refined, cli)) {
        expect(FORM_TOKENS.has(name), `${paletteName(theme)} moved ${name}`).toBe(true);
      }

      // Type: CLI chrome uses the user's terminal face (MonoLisa) and falls
      // back to the bundled Geist Mono stack already named by --font-mono.
      expect(cli.get("--font-ui"), paletteName(theme)).toContain("MonoLisa");
      expect(cli.get("--font-ui"), paletteName(theme)).toContain("Geist Mono");
      expect(refined.get("--font-ui")).not.toContain("MonoLisa");
      expect(refined.get("--font-ui")).not.toContain("Geist Mono");

      // Radius: square, including the aliases that read through.
      for (const name of RADIUS_TOKENS) {
        expect(cli.get(name), `${paletteName(theme)} ${name}`).toBe("0");
      }

      // Depth: these two are the app's only elevation tokens.
      expect(cli.get("--overlay-shadow"), paletteName(theme)).toBe("none");
      expect(cli.get("--composer-lift"), paletteName(theme)).toBe("none");

      // Glass: blur off, saturation neutral.
      expect(cli.get("--material-blur"), paletteName(theme)).toBe("0px");
      expect(cli.get("--material-saturate"), paletteName(theme)).toBe("100%");

      // The fills resolve to grounds this palette already declares, not to new
      // colours: --chrome-bg and --surface, both opaque, both already in the
      // contrast harness's surface matrix.
      expect(cli.get("--material-chrome"), paletteName(theme)).toBe(refined.get("--chrome-bg"));
      expect(cli.get("--material-surface"), paletteName(theme)).toBe(refined.get("--surface"));
      expect(cli.get("--material-widget"), paletteName(theme)).toBe(refined.get("--surface"));
      for (const name of ["--dropdown-bg", "--widget-bg", "--menu-bg"]) {
        expect(cli.get(name), `${paletteName(theme)} ${name}`).toBe(refined.get("--surface"));
      }
      for (const name of ["--material-chrome", "--material-surface", "--material-widget"]) {
        expect(cli.get(name), `${paletteName(theme)} ${name} is opaque`)
          .toMatch(/^#[0-9a-f]{6}$/iu);
      }
    }
  });

  it("shares Dark's structure across every palette within each chrome mode", () => {
    for (const chrome of ["refined", "cli"] as const) {
      const baseline = computedTokens("dark", chrome);
      const structure = (tokens: Map<string, string>): Map<string, string> => new Map(
        [...tokens].filter(([name, value]) => !value.includes("#") && /^(--(?:radius|tile|pane|shell|titlebar|row|section-header|strip|control|icon|space|border-w|text|wght|lh|track|motion|ease|material-blur|material-saturate))/u.test(name)),
      );
      for (const theme of PALETTES) {
        const values = computedTokens(theme, chrome);
        expect(structure(values), `${paletteName(theme)} ${chrome}`).toEqual(structure(baseline));
        for (const name of ["--overlay-shadow", "--composer-lift"]) {
          const geometry = (value: string | undefined): string | undefined => value?.replace(/#[0-9a-f]{6,8}/giu, "COLOR");
          expect(geometry(values.get(name)), `${paletteName(theme)} ${name}`)
            .toBe(geometry(baseline.get(name)));
        }
      }
    }
  });

  it("keeps text legible on the grounds the collapse exposes", () => {
    // The claim the collapse rests on, measured rather than asserted: the fills
    // land on pairs the harness already covers, so no new pair is introduced.
    // The theme-file palettes mix their colours; theme-files.test.ts measures them.
    for (const theme of PALETTES.filter(palette => !palette?.startsWith("custom-"))) {
      const cli = computedTokens(theme, "cli");
      for (const surface of ["--material-surface", "--material-widget", "--material-chrome"]) {
        for (const text of ["--fg", "--fg-strong", "--fg-muted", "--fg-dim"]) {
          const ratio = contrastRatio(cli.get(text) ?? "", cli.get(surface) ?? "");
          expect(ratio, `${paletteName(theme)} ${text} on ${surface}`).toBeGreaterThanOrEqual(4.5);
        }
      }
    }
  });

  it("declares the form axis after every palette so it wins the tie", () => {
    // :root[data-chrome=…] and :root[data-theme=…] have identical specificity,
    // so source order is the only reason CLI beats a frosted palette's blur.
    const chromeRules = RULES.filter(rule => rule.selector.includes("data-chrome"));
    const themeRules = RULES.filter(rule => /:root\[data-theme=/u.test(rule.selector));
    expect(chromeRules.length).toBeGreaterThan(0);
    expect(themeRules.length).toBeGreaterThan(0);
    expect(Math.min(...chromeRules.map(rule => rule.index)))
      .toBeGreaterThan(Math.max(...themeRules.map(rule => rule.index)));
  });

  it("tightens chrome geometry so the workspace reads as a console", () => {
    const refined = computedTokens(null, "refined");
    const cli = computedTokens(null, "cli");
    expect(Number.parseInt(cli.get("--pane-left") ?? "", 10))
      .toBeLessThan(Number.parseInt(refined.get("--pane-left") ?? "", 10));
    expect(Number.parseInt(cli.get("--row-h") ?? "", 10))
      .toBeLessThan(Number.parseInt(refined.get("--row-h") ?? "", 10));
    expect(Number.parseInt(cli.get("--titlebar-h") ?? "", 10))
      .toBeLessThanOrEqual(Number.parseInt(refined.get("--titlebar-h") ?? "", 10));
    expect(cli.get("--row-h")).toBe(cli.get("--control-h"));
  });

  it("tightens the type but never the hit target", () => {
    // The form axis is allowed to take height out of ROWS. It is not allowed to
    // take a pointer target below WCAG 2.5.8's 24px, which this sheet cites by
    // name at the pane controls — a chrome preference is not a reason to shrink
    // a button. Both of these did: the transcript's icon buttons to 20px and
    // the sidebar's top-level destinations to 22px.
    const cli = computedTokens(null, "cli");
    expect(cli.get("--control-h")).toBe("24px");
    // Deleted rather than restated: Conversation.tsx already draws .conv-iconbtn
    // at exactly 24px, so any CLI size override here is either a shrink or a
    // no-op. Both selector forms are refused, since the rule was written
    // unscoped and could come back scoped.
    expect(RULES.filter(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".conv-iconbtn")
      && (candidate.declarations.has("height") || candidate.declarations.has("width")),
    )).toEqual([]);
    // Two CLI rules carry this selector (one sets padding, one the metrics), so
    // the height is read from the merged declarations rather than the first hit.
    const nav = new Map(RULES
      .filter(candidate => candidate.selector === ':root[data-chrome="cli"] .sidebar-nav-button')
      .flatMap(candidate => [...candidate.declarations]));
    expect(nav.get("height")).toBe("var(--control-h)");
    // The type still tightens — that is the half of the rule that was right.
    expect(nav.get("font-size")).toBe("var(--text-ui)");
  });

  it("turns the prompt into a terminal rule, not a lifted card", () => {
    // Claude Code frames the input with two hairlines, not a boxed card.
    const sheet = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-composer');
    expect(sheet?.declarations.get("background")).toBe("transparent");
    expect(sheet?.declarations.get("box-shadow")).toBe("none");
    expect(sheet?.declarations.get("border")).toBe("0");
    const row = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-ta-row');
    expect(row?.declarations.get("border-top"))
      .toBe("var(--border-w) solid var(--border)");
    expect(row?.declarations.get("border-bottom"))
      .toBe("var(--border-w) solid var(--border)");
  });

  it("boxes conversation tiles and names the focused one in weight, not hue", () => {
    const tile = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .workspace-tile');
    expect(tile?.declarations.get("box-shadow"))
      .toBe("inset 0 0 0 var(--border-w) var(--border)");
    // The accent is this app's scarcest signal and it is spent on the approval
    // card. CLI painted the focused pane's title with it, so on a four-pane
    // workspace the orange word said "focused" in the same hue that elsewhere
    // says "you are blocked". Focus is the ramp step and the ring; no override.
    expect(RULES.some(candidate => candidate.selector
      === ':root[data-chrome="cli"] .workspace-tile.is-selected .workspace-tile-name'))
      .toBe(false);
    const name = RULES.find(candidate => candidate.selector === ".workspace-tile-name");
    expect(name?.declarations.get("color")).toBe("var(--fg-muted)");
    // (0,3,0) beats the base (0,1,0) in both chromes, so this is what a focused
    // pane's title actually computes to.
    const selected = RULES.find(candidate =>
      candidate.selector === ".workspace-tile.is-selected .workspace-tile-name");
    expect(selected?.declarations.get("color")).toBe("var(--fg-strong)");
    expect(selected?.declarations.get("font-weight")).toBe("var(--wght-strong)");
    // And CLI adds nothing to the head: the base rule already fills it
    // --editor-bg, so the override was a second way to say the same thing.
    expect(RULES.some(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .workspace-tile-head')).toBe(false);
  });

  it("does not hang generated content on the live session pickers", () => {
    // A `::before` on `.conv-pills > *` lands on AppSelect/EffortBar roots.
    // Those are inline-flex with min-width:0, so the pipe became the only
    // leftover and the Access trigger collapsed. Separators stay in gap.
    for (const rule of RULES) {
      if (!rule.selector.includes("conv-pills")) continue;
      expect(rule.declarations.has("content"), rule.selector).toBe(false);
    }
    const pickers = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-config-select');
    expect(pickers?.declarations.get("flex-shrink")).toBe("0");
    expect(pickers?.declarations.get("min-width")).toBe("max-content");
    const value = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-config-select .app-select-value');
    expect(value?.declarations.get("overflow")).toBe("visible");
    expect(value?.declarations.get("max-width")).toBe("none");
    const modelTrigger = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-model-select .app-select-trigger');
    expect(modelTrigger?.declarations.get("text-transform")).toBe("none");
    const configTriggers = RULES.filter(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".conv-config-select .app-select-trigger"));
    for (const rule of configTriggers) {
      expect(rule.declarations.get("text-transform"), rule.selector).toBeUndefined();
    }
    // And nothing about the `·` separators: ConfigPills returns its CLI session
    // lines before `parts` — the only place a .conv-pill-sep is built — is ever
    // used, so a CLI rule for them styled an element this chrome never mounts.
    expect(RULES.some(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-pill-sep')).toBe(false);
    const chevron = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".app-select-chevron"));
    expect(chevron?.declarations.get("display")).toBe("none");
    const icons = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".conv-controls > .conv-iconbtn"));
    expect(icons?.declarations.get("display")).toBe("none");
  });

  it("flattens the steering queue into a terminal list", () => {
    const card = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .steering-queue');
    expect(card?.declarations.get("background")).toBe("transparent");
    expect(card?.declarations.get("border")).toBe("0");
    expect(card?.declarations.get("font-size")).toBe("var(--text-h1)");
    const caret = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .steering-queue-caret');
    expect(caret?.declarations.get("display")).toBe("inline-block");
    const acts = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .steering-queue-cli-act');
    expect(acts?.declarations.get("display")).toBe("inline");
    const icons = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .steering-queue-iconbtn .codicon');
    expect(icons?.declarations.get("display")).toBe("none");
    const contextIcon = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .steering-queue-context .codicon');
    expect(contextIcon?.declarations.get("display")).toBe("none");
    const errorIcon = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-queue-error .codicon');
    expect(errorIcon?.declarations.get("display")).toBe("none");
    const sendQueue = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-send-queue');
    expect(sendQueue?.declarations.get("font-size")).toBe("var(--text-h1)");
    expect(sendQueue?.declarations.get("text-transform")).toBe("lowercase");
  });

  it("turns slash-command autofill into a TUI list", () => {
    const menu = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .provider-command-menu');
    expect(menu?.declarations.get("background")).toBe("var(--editor-bg)");
    expect(menu?.declarations.get("border")).toBe("0");
    expect(menu?.declarations.get("box-shadow")).toBe("none");
    expect(menu?.declarations.get("animation")).toBe("none");
    expect(menu?.declarations.get("font-size")).toBe("var(--text-h1)");
    expect(menu?.declarations.get("padding")).toBe("8px 0 10px");
    const head = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .provider-command-menu-head');
    expect(head?.declarations.get("display")).toBe("none");
    const caret = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .provider-command-option.is-active::before');
    expect(caret?.declarations.get("content")).toBe("\">\"");
    const active = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".provider-command-option:hover")
      && candidate.selector.includes(".provider-command-option.is-active"));
    expect(active?.declarations.get("background")).toBe("transparent");
    const option = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .provider-command-option');
    expect(option?.declarations.get("padding")).toBe("5px 12px");
    expect(option?.declarations.get("gap")).toBe("16px");
    const name = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .provider-command-name');
    expect(name?.declarations.get("color")).toBe("var(--accent-text)");
  });

  it("turns the effort pop-out into a named TUI ladder", () => {
    const hidden = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".effort-track-shell"));
    expect(hidden?.declarations.get("display")).toBe("none");
    const ladder = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .effort-cli-ladder');
    expect(ladder?.declarations.get("display")).toBe("flex");
    const opt = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .effort-cli-opt.is-current::before');
    expect(opt?.declarations.get("content")).toBe("\">\"");
    expect(css).toMatch(/@keyframes\s+effort-cli-hue\b/u);
    const word = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .effort-picker-maximum .effort-value-current');
    expect(word?.declarations.get("color")).toBe("var(--working-spark, var(--accent-text))");
    expect(word?.declarations.has("animation")).toBe(false);
    const spark = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .effort-picker-celebrate.effort-picker-maximum .effort-value-current::before');
    expect(spark?.declarations.get("content")).toBe("\"✶\"");
    expect(spark?.declarations.get("animation")).toMatch(/cli-glyph/u);
    expect(spark?.declarations.get("animation")).not.toMatch(/infinite/u);
    expect(css).toMatch(/@keyframes\s+effort-cli-bar\b/u);
    expect(css).toMatch(/@keyframes\s+effort-cli-pulse\b/u);
    const rainbow = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(":not([data-provider=\"codex\"])")
      && candidate.selector.includes(".effort-picker-celebrate")
      && candidate.selector.includes(".conv-ta-row::before"));
    expect(rainbow?.declarations.get("animation")).toMatch(/effort-cli-bar/u);
    expect(rainbow?.declarations.get("animation")).not.toMatch(/infinite/u);
    const pulse = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".conv[data-provider=\"codex\"]")
      && candidate.selector.includes(".effort-picker-celebrate")
      && candidate.selector.includes(".conv-ta-row::before"));
    expect(pulse?.declarations.get("animation")).toMatch(/effort-cli-pulse/u);
    expect(pulse?.declarations.get("background")).toBe("var(--mode-auto)");
    expect(pulse?.declarations.get("animation")).not.toMatch(/infinite/u);
    expect(css).toMatch(/@keyframes\s+effort-cli-disk\b/u);
    expect(css).toMatch(/@keyframes\s+cli-glyph-grok-effort\b/u);
    const disk = RULES.find(candidate =>
      candidate.selector.includes('[data-provider="grok"]')
      && candidate.selector.includes(".effort-picker-celebrate.effort-picker-xhigh")
      && candidate.selector.includes(".conv-ta-row::before")
      && candidate.declarations.has("animation"));
    expect(disk?.declarations.get("animation")).toMatch(/effort-cli-disk/u);
    expect(disk?.declarations.get("animation")).not.toMatch(/infinite/u);
    expect(disk?.declarations.get("background")).toContain("var(--editor-bg)");
    const hole = RULES.find(candidate =>
      candidate.selector.includes('[data-provider="grok"]')
      && candidate.selector.includes(".effort-picker-celebrate.effort-picker-xhigh .effort-value-current::before"));
    expect(hole?.declarations.get("content")).toBe("\"◉\"");
    expect(hole?.declarations.get("animation")).toMatch(/cli-glyph-grok-effort/u);
    expect(hole?.declarations.get("animation")).not.toMatch(/infinite/u);
    const restingHairline = RULES.find(candidate =>
      candidate.selector.includes(".conv-composer:has(.effort-picker-maximum)")
      && !candidate.selector.includes("celebrate")
      && candidate.declarations.has("animation"));
    expect(restingHairline).toBeUndefined();
  });

  it("un-fills the send target so it is a glyph, not a circular button", () => {
    const rule = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-send');
    expect(rule?.declarations.get("background")).toBe("transparent");
    expect(rule?.declarations.get("color")).toBe("var(--fg-dim)");
  });

  it("sets the conversation to the user's Ghostty type size and padding", () => {
    const pane = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv');
    expect(pane?.declarations.get("font-size")).toBe("var(--text-h1)");
    expect(pane?.declarations.get("line-height")).toBe("1.3");
    expect(pane?.declarations.get("--conv-gutter")).toBe("12px");
    const prose = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".conv-text")
      && candidate.selector.includes(".conv-markdown"));
    expect(prose?.declarations.get("letter-spacing")).toBe("0");
    expect(prose?.declarations.get("line-height")).toBe("1.5");
    expect(prose?.declarations.get("font-weight")).toBe("var(--wght-prose)");
    const list = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-list');
    expect(list?.declarations.get("padding")).toBe("10px 12px");
  });

  it("draws a blinking block caret in the prompt", () => {
    const rule = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".conv-ta")
      && (candidate.declarations.has("caret-shape")
        || candidate.declarations.has("caret-color")));
    expect(rule?.declarations.get("caret-shape")).toBe("block");
    expect(rule?.declarations.get("caret-color")).toBe("var(--focus)");
  });

  it("lights the ❯ on the focused pane only, and dims both session lines", () => {
    const find = (selector: string) =>
      RULES.find(candidate => candidate.selector === selector);
    // Four panes mean four prompts, and four accented carets would be four
    // claims of "type here". The accent is the focus indicator, so only the
    // selected tile gets it.
    expect(find(':root[data-chrome="cli"] .conv-prompt-mark')?.declarations.get("color"))
      .toBe("var(--fg-faint)");
    expect(find(':root[data-chrome="cli"] .workspace-tile.is-selected .conv-prompt-mark')
      ?.declarations.get("color")).toBe("var(--accent-text)");
    expect(find(':root[data-chrome="cli"] .conv-ta')?.declarations.get("color"))
      .toBe("var(--fg-strong)");

    const line = find(':root[data-chrome="cli"] .conv-session-line');
    expect(line?.declarations.get("align-items")).toBe("center");
    expect(line?.declarations.get("gap")).toBe("6px");
    expect(line?.declarations.get("color")).toBe("var(--fg-dim)");
    expect(find(':root[data-chrome="cli"] .conv-session-model')?.declarations.get("color"))
      .toBe("var(--fg)");
    expect(find(':root[data-chrome="cli"] .conv-session-star')?.declarations.get("color"))
      .toBe("var(--working-spark, var(--fg-dim))");
    // The meter goes to the far end of line one rather than trailing `fast`,
    // and these three declarations are ONE mechanism: `margin-left: auto` can
    // only push into free space. `.conv-pills` is a shrink-to-fit flex item of
    // `.conv-controls` until it is told to grow, and its `align-items:
    // flex-start` shrink-wraps each line inside it until the line is told to
    // stretch. Drop either enabler and the meter silently returns to trailing
    // `fast`, with the margin still in the sheet looking correct.
    expect(find(':root[data-chrome="cli"] .conv-session-line .conv-context-meter')
      ?.declarations.get("margin-left")).toBe("auto");
    const pills = find(':root[data-chrome="cli"] .conv-pills');
    expect(pills?.declarations.get("flex")).toBe("1 1 auto");
    expect(pills?.declarations.get("min-width")).toBe("0");
    expect(line?.declarations.get("align-self")).toBe("stretch");
    expect(find(':root[data-chrome="cli"] .conv-session-plan')?.declarations.get("color"))
      .toBe("var(--fg-dim)");
    // Scoped off its own warning states, and that is the whole point of the
    // :not(): `.conv-session-tokens.is-high` is (0,2,0) in Conversation.tsx's
    // injected sheet, so a bare `:root[data-chrome="cli"] .conv-session-tokens`
    // at (0,3,0) would win and paint a full context window the same quiet grey
    // as an empty one. Both classes sit on the same button element.
    expect(find(':root[data-chrome="cli"] .conv-session-tokens')).toBeUndefined();
    expect(find(
      ':root[data-chrome="cli"] .conv-session-tokens:not(.is-high):not(.is-compacting)')
      ?.declarations.get("color")).toBe("var(--fg-faint)");
    expect(find(':root[data-chrome="cli"] .conv-session-hints')?.declarations.get("color"))
      .toBe("var(--fg-faint)");

    // What the two lines no longer draw. Every pattern is a word boundary, not
    // a prefix: `conv-session-model` and `conv-session-hints` are the classes
    // that stay, and each opens with the characters being refused here.
    for (const gone of [/conv-session-mark\b/u, /conv-session-mode\b/u,
      /conv-session-sep\b/u, /conv-session-hint\b/u]) {
      expect(RULES.some(candidate => gone.test(candidate.selector)), String(gone))
        .toBe(false);
    }
  });

  it("cycles the agentic CLI sparkle glyphs while work is running", () => {
    expect(css).toMatch(/@keyframes\s+cli-glyph\b/u);
    expect(css).toMatch(/@keyframes\s+cli-glyph-codex\b/u);
    expect(css).toMatch(/@keyframes\s+cli-glyph-grok\b/u);
    expect(css).toContain("content: \"·\"");
    expect(css).toContain("content: \"✢\"");
    expect(css).toContain("content: \"✳\"");
    expect(css).toContain("content: \"✶\"");
    expect(css).toContain("content: \"✻\"");
    expect(css).toContain("content: \"✽\"");
    expect(css).toContain("content: \"⠋\"");
    expect(css).toContain("content: \"○\"");
    expect(css).toContain("content: \"◎\"");
    expect(css).toContain("content: \"◉\"");
    expect(css).toContain("content: \"●\"");
    expect(css).not.toMatch(/@keyframes\s+cli-glyph-grok\s*\{[^}]*content:\s*"[▁▂▃▄▅▆▇█]"/u);
    const working = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-working::before');
    expect(working?.declarations.get("animation")).toMatch(/cli-glyph/u);
    expect(working?.declarations.get("color")).toBe("var(--working-spark, var(--status-working))");
    const dot = RULES.find(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .dot-working.dot-pulse::before');
    expect(dot?.declarations.get("animation")).toMatch(/cli-glyph/u);
    expect(dot?.declarations.get("background")).toBe("transparent");
    const codex = RULES.find(candidate =>
      candidate.selector.includes('[data-provider="codex"].conv-working::before'));
    expect(codex?.declarations.get("animation")).toMatch(/cli-glyph-codex/u);
    const grok = RULES.find(candidate =>
      candidate.selector.includes('[data-provider="grok"].conv-working::before'));
    expect(grok?.declarations.get("animation")).toMatch(/cli-glyph-grok/u);
  });

  it("squares the one corner no radius token can express", () => {
    // There is no escape list any more. It was five names zeroed here by hand,
    // and a by-name list is a second place to remember: it still carried
    // .quick-open-match long after Quick Open was deleted. The code chip, the
    // monogram tile and the queue capsule read --radius-control and
    // --radius-pill now, both zeroed at the top of this block, so they square
    // without being named. Send cannot: it is a disc, and 50% is a shape.
    const send = RULES.filter(candidate =>
      candidate.selector === ':root[data-chrome="cli"] .conv-send'
      && candidate.declarations.has("border-radius"));
    expect(send).toHaveLength(1);
    expect(send[0].declarations.get("border-radius")).toBe("0");
    expect(RULES.some(candidate =>
      candidate.selector.startsWith(':root[data-chrome="cli"] :is('))).toBe(false);
    expect(RULES.filter(candidate => candidate.selector.includes("quick-open"))).toEqual([]);
  });

  it("squares the toolbar's segmented control, ground and members alike", () => {
    const squared = RULES.find(candidate =>
      candidate.selector.includes('[data-chrome="cli"]')
      && candidate.selector.includes(".workspace-toolbar-btn"));
    expect(squared?.declarations.get("border-radius")).toBe("0");
    for (const target of [".workspace-icon-btn", ".surface-switch", ".surface-switch-tab"]) {
      expect(squared?.selector, target).toContain(target);
    }
  });

  it("marks the active surface as the one raised tab, edged by a hairline", () => {
    // Porcelain (owner, 2026-09-26): the active tab is the surface colour with
    // a hairline edge. No underline, no bevel, no text shadow.
    const active = RULES.find(candidate => candidate.selector === ".surface-switch-tab.is-active");
    expect(active?.declarations.get("background")).toBe("var(--surface)");
    expect(active?.declarations.get("color")).toBe("var(--fg-strong)");
    expect(active?.declarations.get("box-shadow")).toBe("0 0 0 var(--border-w) var(--border)");
    expect(active?.declarations.has("text-shadow")).toBe(false);
  });

  it("sits the Agent Code Chat labels in a flat recessed track", () => {
    const track = RULES.find(candidate => candidate.selector === ".surface-switch");
    expect(track?.declarations.get("background")).toBe("color-mix(in srgb, var(--fg) 6%, transparent)");
    expect(track?.declarations.get("border")).toBe("0");
    expect(track?.declarations.has("box-shadow")).toBe(false);
    const tab = RULES.find(candidate => candidate.selector === ".surface-switch-tab");
    expect(tab?.declarations.has("text-shadow")).toBe(false);
  });

  it("gives tiles their own geometry, identical in both chromes", () => {
    // The three tile tokens are the one deliberate exception to CLI's square
    // corners, so they are declared once on :root and never re-declared on the
    // chrome axis — which is also what keeps the FORM_TOKENS assertion above
    // true without adding three names to it.
    for (const theme of PALETTES) {
      const refined = computedTokens(theme, "refined");
      const cli = computedTokens(theme, "cli");
      for (const name of ["--tile-radius", "--tile-gap", "--tile-head-h"]) {
        expect(refined.get(name), `${paletteName(theme)} ${name}`).toBe(cli.get(name));
      }
      expect(refined.get("--tile-radius")).toBe("12px");
      expect(refined.get("--tile-gap")).toBe("12px");
      expect(refined.get("--tile-head-h")).toBe("30px");
    }
  });

  it("draws the workspace as spaced, rounded panes with a chromatic focus ring", () => {
    const grid = RULES.find(candidate => candidate.selector === ".workspace-grid");
    expect(grid?.declarations.get("gap")).toBe("var(--tile-gap)");
    expect(grid?.declarations.get("padding")).toBe("var(--tile-gap)");
    expect(grid?.declarations.get("box-sizing")).toBe("border-box");
    const tile = RULES.find(candidate => candidate.selector === ".workspace-tile");
    expect(tile?.declarations.get("border-radius")).toBe("var(--tile-radius)");
    expect(tile?.declarations.get("overflow")).toBe("hidden");
    expect(tile?.declarations.get("box-shadow"))
      .toBe("inset 0 0 0 var(--border-w) var(--border)");
    // The focused pane uses the interaction blue, independently of the warm
    // Interface identity accent and the provider hues inside the pane.
    const selected = RULES.find(candidate =>
      candidate.selector === ".workspace-tile.is-selected");
    expect(selected?.declarations.get("box-shadow"))
      .toBe("inset 0 0 0 var(--border-w) var(--selection-accent)");
    // The count-1 suppression survives, but as "no RING", not "no edge".
    const single = RULES.find(candidate =>
      candidate.selector === ".workspace-grid.workspace-count-1 .workspace-tile.is-selected");
    expect(single?.declarations.get("box-shadow"))
      .toBe("inset 0 0 0 var(--border-w) var(--border)");
    const head = RULES.find(candidate => candidate.selector === ".workspace-tile-head");
    expect(head?.declarations.get("height")).toBe("var(--tile-head-h)");
    expect(head?.declarations.get("background")).toBe("var(--editor-bg)");
    expect(head?.declarations.get("gap")).toBe("8px");
    // …and an opaque fill has nothing to blur through it, so the head is off the
    // frosted list. It stays off: the filter buys nothing and costs a layer.
    const frosted = RULES.find(candidate =>
      candidate.declarations.get("backdrop-filter")
        === "blur(var(--material-blur)) saturate(var(--material-saturate))");
    expect(frosted?.selector).toBe(".is-floating");
    expect(frosted?.selector).not.toContain(".workspace-tile-head");
    const name = RULES.find(candidate => candidate.selector === ".workspace-tile-name");
    // 13px, reached through the ramp: theme-accessibility.test.ts forbids a
    // literal px font-size in the file that declares the ramp, and --text-body
    // is 13px on bare :root in every palette and both chromes.
    expect(name?.declarations.get("font-size")).toBe("var(--text-body)");
    expect(computedTokens(null, "refined").get("--text-body")).toBe("13px");
  });

  it("puts a state dot in the pane header and never hides its controls", () => {
    const dot = RULES.find(candidate =>
      candidate.selector === ".workspace-agent .workspace-tile-head::before");
    expect(dot?.declarations.get("content")).toBe('""');
    expect(dot?.declarations.get("width")).toBe("6px");
    expect(dot?.declarations.get("border-radius")).toBe("50%");
    // Idle draws nothing (owner, 2026-09-12); the column stays for the title's left edge.
    expect(dot?.declarations.get("background")).toBe("transparent");
    const failed = RULES.find(candidate => candidate.selector
      === '.workspace-agent[data-status="failed"] .workspace-tile-head::before');
    expect(failed?.declarations.get("background")).toBe("var(--status-failed)");
    // The live head (owner, 2026-10-07): a working or waiting thread says so
    // with a 2px line on the head's bottom edge, not a dot — still in the
    // project's hues while it works (it swept until the owner asked for less
    // motion), still in amber while it waits.
    for (const state of ["working", "waiting"]) {
      expect(RULES.some(candidate => candidate.selector
        === `.workspace-agent[data-status="${state}"] .workspace-tile-head::before`), state).toBe(false);
    }
    const line = RULES.find(candidate => candidate.selector === ".workspace-agent .workspace-tile-head::after");
    expect(line?.declarations.get("height")).toBe("2px");
    expect(line?.declarations.get("bottom")).toBe("calc(-1 * var(--border-w))");
    const running = RULES.find(candidate => candidate.selector
      === '.workspace-agent[data-status="working"] .workspace-tile-head::after');
    expect(running?.declarations.has("animation")).toBe(false);
    expect(running?.declarations.get("width")).toBe("100%");
    expect(running?.declarations.get("background")).toBe("color-mix(in srgb, var(--fg-dim) 45%, transparent)");
    const held = RULES.find(candidate => candidate.selector
      === '.workspace-agent[data-status="waiting"] .workspace-tile-head::after');
    expect(held?.declarations.get("background")).toBe("color-mix(in srgb, var(--tile-waiting) 70%, transparent)");
    expect(held?.declarations.has("animation")).toBe(false);
    // Controls are part of the header now, not a hover reveal.
    const actions = RULES.find(candidate =>
      candidate.selector === ".workspace-tile-actions");
    expect(actions?.declarations.get("opacity")).toBeUndefined();
    expect(actions?.declarations.get("gap")).toBe("10px");
    expect(RULES.some(candidate =>
      candidate.selector.includes(".workspace-tile:hover .workspace-tile-actions"))).toBe(false);
    // And they are ACTIONABLE, so they sit in the quiet-control tier, not the
    // decorative one. --fg-faint is documented (theme.css:88-95) as legal only
    // for decoration, aria-hidden glyphs and disabled controls; split, maximize
    // and close are none of those, and at --fg-faint these three were the
    // faintest thing in the header at 2.18-4.18:1.
    expect(actions?.declarations.get("color")).toBe("var(--fg-muted)");
    // A refused split has to LOOK refused: `color: inherit` above ties with
    // .workspace-icon-btn:disabled and wins on source order, so without this the
    // ceiling's + is pixel-identical to a live one. ONE dimming step, though —
    // --fg-faint at opacity 0.5 measured 1.51-1.98:1, which is not a refusal
    // anybody can see. The resting tier moving up to --fg-muted is what makes
    // the single step enough to read as different.
    const off = RULES.find(candidate =>
      candidate.selector === ".workspace-tile-actions .workspace-icon-btn:disabled");
    expect(off?.declarations.get("opacity")).toBeUndefined();
    expect(off?.declarations.get("color")).toBe("var(--fg-faint)");
    // No mode picker beside them: the access mode moved to the composer's
    // access chip in the Threads redesign (owner, 2026-10-06).
    expect(RULES.some(candidate => candidate.selector.includes(".workspace-tile-access"))).toBe(false);
    // Task 1 made both head states --editor-bg, so the selected-head override is
    // dead weight that still wins the cascade in refined chrome. It stays gone —
    // and so does its CLI twin, which `includes` is here to catch.
    expect(RULES.some(candidate =>
      candidate.selector.includes(".workspace-tile.is-selected .workspace-tile-head")
      && candidate.declarations.has("background"))).toBe(false);
  });

  it("speaks Claude Code's transcript vocabulary in bare CLI chrome", () => {
    // By selector-list part, so a rule shared by the tool, run-summary and
    // file rows answers for each of them.
    const find = ruleFor;

    // Assistant prose: a bullet in the margin, drawn out of flow. .conv-text's
    // only child is the BLOCK .conv-markdown, so an inline ::before would be
    // wrapped in an anonymous block and draw on a line of its own; and
    // text-indent INHERITS, so a hanging indent here would re-apply to every
    // paragraph, list item, heading and quote inside the markdown. Absolute in
    // the padding is neither.
    const text = find(':root[data-chrome="cli"] .conv-text::before');
    expect(text?.declarations.get("content")).toBe('"●"');
    expect(text?.declarations.get("color")).toBe("var(--fg-strong)");
    expect(text?.declarations.get("position")).toBe("absolute");
    expect(text?.declarations.get("left")).toBe("0");
    const textBlock = find(':root[data-chrome="cli"] .conv-text');
    expect(textBlock?.declarations.get("position")).toBe("relative");
    expect(textBlock?.declarations.get("padding-left")).toBe("1.1em");
    expect(textBlock?.declarations.has("text-indent")).toBe(false);

    // The user's turn: a band with an accent caret, no bubble.
    const user = find(':root[data-chrome="cli"] .conv-turn-user');
    expect(user?.declarations.get("background"))
      .toBe("color-mix(in srgb, var(--row-selected) 50%, transparent)");
    expect(user?.declarations.get("border-radius")).toBe("3px");
    expect(user?.declarations.get("max-width")).toBe("none");
    // A band, not a bubble: the shipped turn's 1px box goes with the fill.
    expect(user?.declarations.get("border")).toBe("0");
    // The caret hangs on the TEXT, not on the turn: .conv-turn-head is the
    // turn's first child, so a ::before on the turn itself draws above the "You"
    // row and the attached images rather than beside the words.
    const userText = find(':root[data-chrome="cli"] .conv-turn-user .conv-turn-text');
    expect(userText?.declarations.get("display")).toBe("block");
    expect(userText?.declarations.get("padding-left")).toBe("1.1em");
    expect(userText?.declarations.get("text-indent")).toBe("-1.1em");
    const userMark = find(
      ':root[data-chrome="cli"] .conv-turn-user .conv-turn-text::before');
    expect(userMark?.declarations.get("content")).toBe('"› "');
    expect(userMark?.declarations.get("color")).toBe("var(--accent-text)");
    expect(find(':root[data-chrome="cli"] .conv-turn-user::before')).toBeUndefined();

    // Tool rows: a state-coloured bullet in place of the codicon, and the
    // summary on its own ⎿ line.
    const kind = find(':root[data-chrome="cli"] .conv-tool-row .conv-kind');
    expect(kind?.declarations.get("display")).toBe("none");
    const bullet = find(':root[data-chrome="cli"] .conv-tool-row::before');
    expect(bullet?.declarations.get("content")).toBe('"●"');
    expect(bullet?.declarations.get("color")).toBe("var(--diff-added)");
    const running = find(
      ':root[data-chrome="cli"] .conv-tool-card:has(.conv-state-run) .conv-tool-row::before');
    expect(running?.declarations.get("color")).toBe("var(--status-working)");
    const failed = find(
      ':root[data-chrome="cli"] .conv-tool-card:has(.conv-state-fail) .conv-tool-row::before');
    expect(failed?.declarations.get("color")).toBe("var(--status-failed)");
    const row = find(':root[data-chrome="cli"] .conv-tool-row');
    expect(row?.declarations.get("flex-wrap")).toBe("wrap");
    // Wrapping is only half of it: .conv-row is height:var(--row-h) in
    // Conversation.tsx and sits inside .conv-card's overflow:hidden, so without
    // releasing the height the ⎿ line is drawn and then clipped away.
    expect(row?.declarations.get("height")).toBe("auto");
    expect(row?.declarations.get("min-height")).toBe("var(--row-h)");
    // No align-items here: .conv-row already centres, and centring is what a
    // one-line row wants. Overriding it to flex-start de-centres every row that
    // never wraps to buy nothing on the ones that do.
    expect(row?.declarations.has("align-items")).toBe(false);
    const summary = find(':root[data-chrome="cli"] .conv-tool-row .conv-sum');
    expect(summary?.declarations.get("flex-basis")).toBe("100%");
    // Order, or the state and the chevron are pushed onto a THIRD line: a
    // flex-basis:100% item in the middle of the row takes line 2 by itself.
    expect(summary?.declarations.get("order")).toBe("1");
    expect(summary?.declarations.get("color")).toBe("var(--fg-muted)");
    const summaryMark = find(':root[data-chrome="cli"] .conv-tool-row .conv-sum::before');
    expect(summaryMark?.declarations.get("content")).toBe('"⎿ "');
    expect(summaryMark?.declarations.get("color")).toBe("var(--fg-faint)");

    // A FOLDED RUN's ⎿ line: the first call's own result. Conversation.tsx's
    // sheet keeps it out of refined chrome, so this rule is the only thing that
    // puts it on screen anywhere.
    const runResult = find(':root[data-chrome="cli"] .conv-run-result');
    expect(runResult?.declarations.get("display")).toBe("block");

    // A file-change row is a tool row that happens to name a path, and reads as
    // one: same bullet, same second line, not `[icon] Updating ⎿ path` inline.
    // The two-line treatment is scoped to .conv-filerow — the ONLY diff row that
    // carries a .conv-name — because the other two shapes have nothing to leave
    // on line 1, so wrapping their summary drew a line holding just the bullet.
    const diffRow = find(':root[data-chrome="cli"] .conv-diffrow.conv-filerow');
    expect(diffRow?.declarations.get("flex-wrap")).toBe("wrap");
    expect(diffRow?.declarations.get("height")).toBe("auto");
    expect(diffRow?.declarations.get("min-height")).toBe("var(--row-h)");
    expect(find(':root[data-chrome="cli"] .conv-diffrow')).toBeUndefined();
    const diffSum = find(':root[data-chrome="cli"] .conv-diffrow.conv-filerow .conv-sum');
    expect(diffSum?.declarations.get("flex-basis")).toBe("100%");
    expect(diffSum?.declarations.get("order")).toBe("1");
    expect(diffSum?.declarations.get("padding-left")).toBe("1.1em");
    expect(diffSum?.declarations.get("color")).toBe("var(--fg-muted)");
    expect(find(':root[data-chrome="cli"] .conv-diffrow.conv-filerow .conv-sum::before')
      ?.declarations.get("content")).toBe('"⎿ "');
    // Nothing lands on a bare .conv-diffrow's summary, so the plain rows stay
    // one line with their own metrics.
    expect(find(':root[data-chrome="cli"] .conv-diffrow .conv-sum')).toBeUndefined();
    expect(find(':root[data-chrome="cli"] .conv-diffrow .conv-sum::before')).toBeUndefined();

    // The bullet and the kind glyph DO stay on all three shapes: every diff row
    // is a transcript line in this chrome.
    expect(find(':root[data-chrome="cli"] .conv-diffrow .conv-kind')
      ?.declarations.get("display")).toBe("none");
    const diffBullet = find(':root[data-chrome="cli"] .conv-diffrow::before');
    expect(diffBullet?.declarations.get("content")).toBe('"●"');
    expect(diffBullet?.declarations.get("color")).toBe("var(--diff-added)");
    // …and green means it LANDED. A still-running or failed edit recolours it
    // from the state span the row already carries, exactly as the tool rows do.
    expect(find(':root[data-chrome="cli"] .conv-diffrow:has(.conv-state-run)::before')
      ?.declarations.get("color")).toBe("var(--status-working)");
    expect(find(':root[data-chrome="cli"] .conv-diffrow:has(.conv-state-fail)::before')
      ?.declarations.get("color")).toBe("var(--status-failed)");

    // The row navigates, so it keeps refined's pointer affordance. It ties with
    // the .conv-filerow summary rule on specificity, so source order is the
    // whole mechanism: these two must come after it in the sheet.
    const hoverPath = find(':root[data-chrome="cli"] .conv-diffrow:hover .conv-sum');
    expect(hoverPath?.declarations.get("color")).toBe("var(--accent)");
    expect(find(':root[data-chrome="cli"] .conv-diffrow:focus-visible .conv-sum')
      ?.declarations.get("color")).toBe("var(--accent)");
    expect(hoverPath?.index ?? -1).toBeGreaterThan(diffSum?.index ?? 0);

    // The session's opening block, and only one opening: the hero below it
    // would print the app's wordmark three lines under the engine's name, and
    // the shared empty-state rule hides that wordmark in every chrome. Its
    // "No messages yet." line is the half that is still news and stays.
    const welcome = find(':root[data-chrome="cli"] .conv-welcome');
    expect(welcome?.declarations.get("display")).toBe("flex");
    expect(welcome?.declarations.get("flex-direction")).toBe("column");
    expect(find(':root[data-chrome="cli"] .conv-welcome-title')?.declarations.get("color"))
      .toBe("var(--fg-strong)");
    // Three tiers down the block, so the three lines are readable as a rank:
    // who (--fg-strong), what it is running (--fg-dim), where (--fg-faint).
    expect(find(':root[data-chrome="cli"] .conv-welcome-detail')?.declarations.get("color"))
      .toBe("var(--fg-dim)");
    expect(find(':root[data-chrome="cli"] .conv-welcome-path')?.declarations.get("color"))
      .toBe("var(--fg-faint)");
    expect(css).toContain(".conv-hero-wordmark,\n.empty-wordmark { display: none; }");
    expect(find(':root[data-chrome="cli"] .conv-welcome ~ .conv-hero .conv-hero-wordmark'))
      .toBeUndefined();
    expect(find(':root[data-chrome="cli"] .conv-hero-copy')).toBeUndefined();

    // The busy line holds its own clock in CLI, and it reticks every second:
    // tabular figures are what stop the sentence twitching from 8s to 9s.
    const busy = find(':root[data-chrome="cli"] .conv-working-label');
    expect(busy?.declarations.get("font-variant-numeric")).toBe("tabular-nums");

    // ● Turn finished. The row is a flex line, so the bullet is a flex ITEM and
    // a trailing space inside its content would be trimmed off — the gap is
    // what keeps it from reading as "●Turn finished".
    const clock = find(':root[data-chrome="cli"] .conv-turn-clock');
    expect(clock?.declarations.get("justify-content")).toBe("flex-start");
    expect(clock?.declarations.get("gap")).toBe("4px");
    const clockMark = find(':root[data-chrome="cli"] .conv-turn-clock::before');
    expect(clockMark?.declarations.get("content")).toBe('"●"');
    expect(clockMark?.declarations.get("color")).toBe("var(--diff-added)");

    // ✗ and the engine's own sentence, on ONE line. .conv-notice is already a
    // flex row, so the mark is its first item; a display:block anywhere here
    // wraps that mark in an anonymous block and strands a bare ✗ above the text.
    const fail = find(':root[data-chrome="cli"] .conv-notice-fail');
    expect(fail?.declarations.get("color")).toBe("var(--status-failed)");
    expect(fail?.declarations.has("display")).toBe(false);
    expect(find(':root[data-chrome="cli"] .conv-notice-fail::before')
      ?.declarations.get("content")).toBe('"✗"');
    const failText = find(':root[data-chrome="cli"] .conv-notice-fail > span');
    expect(failText?.declarations.get("color")).toBe("var(--fg-muted)");
    expect(failText?.declarations.has("display")).toBe(false);
    expect(failText?.declarations.has("padding-left")).toBe(false);
    expect(find(':root[data-chrome="cli"] .conv-notice-fail > span::before')).toBeUndefined();

    // A tiled transcript is 13px; a single pane keeps its 19px. The size is
    // named, not typed: theme-accessibility.test.ts refuses a literal px
    // font-size in the file that declares the ramp, and --text-body is 13px.
    const tiled = find(
      ':root[data-chrome="cli"] .workspace-grid:not(.workspace-count-1) .conv');
    expect(tiled?.declarations.get("font-size")).toBe("var(--text-body)");
    expect(computedTokens(null, "cli").get("--text-body")).toBe("13px");
    expect(tiled?.declarations.get("line-height")).toBe("1.5");
  });
});

describe("refined stays exactly what shipped", () => {
  it("is the untouched cascade: no rule anywhere selects it", () => {
    const chromeSelectors = RULES
      .flatMap(rule => rule.selector.split(","))
      .map(part => part.trim())
      .filter(part => part.includes("data-chrome"));
    expect(chromeSelectors.length).toBeGreaterThan(0);
    for (const selector of chromeSelectors) {
      expect(selector, selector).toContain('[data-chrome="cli"]');
    }
  });

  it("computes token-for-token identically with and without the attribute", () => {
    for (const theme of PALETTES) {
      const before = computedTokens(theme, null);
      const after = computedTokens(theme, "refined");
      expect(differingTokens(before, after), paletteName(theme)).toEqual([]);
      expect(after.size).toBeGreaterThan(60);
    }
  });

  it("renders a pane byte-identically in both modes, because form is CSS-only", () => {
    const render = (chrome: AppPreferences["uiChrome"]): string =>
      renderToStaticMarkup(createElement(SettingsPage, settingsProps({
        preferences: { ...DEFAULT_APP_PREFERENCES, uiChrome: chrome },
      })));
    // The picker's own reported value is the single permitted difference. No
    // component branches on the mode, so nothing else can regress when it is
    // off — the whole form change lives in the stylesheet.
    const normalize = (html: string): string => html
      .replace(/Interface style: (?:Refined|Bare CLI)/gu, "Interface style: X")
      .replace(/title="(?:Refined|Bare CLI) — [^"]*"/gu, 'title="X"')
      .replace(/>(?:Refined|Bare CLI)</gu, ">X<");
    expect(normalize(render("cli"))).toBe(normalize(render("refined")));
    expect(render("cli")).not.toBe(render("refined"));
  });
});

describe("the chrome preference", () => {
  it("offers exactly two values and defaults to the shipped harness", () => {
    expect(UI_CHROME_PREFERENCES).toEqual(["refined", "cli"]);
    expect(DEFAULT_APP_PREFERENCES.uiChrome).toBe("refined");
  });

  it("falls back to refined for anything it does not recognise", () => {
    for (const value of ["cli ", "CLI", "bare", "", 0, null, undefined, {}, ["cli"]]) {
      expect(isUiChromePreference(value), JSON.stringify(value) ?? "undefined").toBe(false);
    }
    expect(isUiChromePreference("cli")).toBe(true);
    expect(isUiChromePreference("refined")).toBe(true);
    expect(normalizeAppPreferences({ uiChrome: "terminal" }).uiChrome).toBe("refined");
    expect(parseAppPreferences('{"uiChrome":"terminal"}').uiChrome).toBe("refined");
    expect(parseAppPreferences('{"uiChrome":7}').uiChrome).toBe("refined");
    expect(parseAppPreferences("{not json").uiChrome).toBe("refined");
    // Corruption in one field must not take a valid neighbour with it.
    expect(parseAppPreferences('{"uiChrome":"terminal","uiFont":"serif"}'))
      .toEqual({ ...DEFAULT_APP_PREFERENCES, uiFont: "serif" });
    expect(parseAppPreferences('{"uiChrome":"cli"}').uiChrome).toBe("cli");
  });
});

/** A root-element stand-in; this suite has no DOM and does not want one. */
function withRootElement<T>(run: (dataset: Record<string, string>) => T): T {
  const dataset: Record<string, string> = {};
  const host = globalThis as unknown as { document?: unknown };
  const previous = host.document;
  host.document = { documentElement: { dataset } };
  try {
    applySettingsChromeLock(false);
    return run(dataset);
  } finally {
    applySettingsChromeLock(false);
    if (previous === undefined) delete host.document;
    else host.document = previous;
  }
}

describe("the attribute reaches the document", () => {
  it("paints both appearance attributes at STARTUP from stored preferences", () => {
    const dataset = withRootElement(seen => {
      const preferences = initializeAppPreferences({
        getItem: () => JSON.stringify({ uiChrome: "cli", uiFont: "serif" }),
      });
      expect(preferences.uiChrome).toBe("cli");
      return seen;
    });
    // Not "after you toggle it": this is the first read of the durable value.
    expect(dataset.chrome).toBe("cli");
    expect(dataset.font).toBe("serif");
  });

  it("paints refined at startup when the stored value is corrupt", () => {
    for (const raw of ['{"uiChrome":"terminal"}', "{not json", "null", ""]) {
      const dataset = withRootElement(seen => {
        initializeAppPreferences({ getItem: () => raw });
        return seen;
      });
      expect(dataset.chrome, raw).toBe("refined");
    }
  });

  it("paints refined at startup when storage itself is unavailable", () => {
    const dataset = withRootElement(seen => {
      initializeAppPreferences({ getItem: () => { throw new Error("blocked"); } });
      return seen;
    });
    expect(dataset.chrome).toBe("refined");
  });

  it("repaints on change, so the toggle is immediate", () => {
    const dataset = withRootElement(seen => {
      applyAppearancePreferences({ ...DEFAULT_APP_PREFERENCES, uiChrome: "cli" });
      expect(seen.chrome).toBe("cli");
      applyAppearancePreferences({ ...DEFAULT_APP_PREFERENCES });
      return seen;
    });
    expect(dataset.chrome).toBe("refined");
  });

  it("writes the attribute theme.css actually selects on", () => {
    const dataset = withRootElement(seen => {
      applyUiChrome("cli");
      return seen;
    });
    // `dataset.chrome` is data-chrome, which is the selector parsed above.
    expect(dataset.chrome).toBe("cli");
    expect(RULES.some(rule => rule.selector === ':root[data-chrome="cli"]')).toBe(true);
  });

  it("keeps Settings on refined chrome without rewriting the stored preference", () => {
    const dataset = withRootElement(seen => {
      applyUiChrome("cli");
      applySettingsChromeLock(true);
      expect(seen.chrome).toBe("refined");
      applyAppearancePreferences({ ...DEFAULT_APP_PREFERENCES, uiChrome: "cli" });
      expect(seen.chrome).toBe("refined");
      applySettingsChromeLock(false);
      return seen;
    });
    expect(dataset.chrome).toBe("cli");
  });
});

describe("App wires the preference on the startup path", () => {
  it("applies it from the state INITIALISER, not from an effect", () => {
    expect(app)
      .toMatch(/useState<AppPreferences>\(\(\) =>\s*initializeAppPreferences\(localStorage\)\)/u);
    expect(app).toContain("applyAppearancePreferences(next)");
    // One seam only: a surviving single-attribute call is exactly how a second
    // preference gets forgotten on the startup path.
    expect(app).not.toContain("applyUiFont(");
    expect(app).not.toContain("applyUiChrome(");
    // Settings is never a console: the page (and its portals) lock refined
    // chrome for the duration, then restore the stored preference.
    expect(app).toContain("applySettingsChromeLock(true)");
    expect(app).toContain("applySettingsChromeLock(false)");
    expect(app).toContain("applySettingsChromeLock(settingsOpen)");
    expect(app).toContain("pairThemeWithChrome(");
    expect(app).toContain("setAppearancePreference(paired.theme)");
  });
});

describe("the Settings control", () => {
  it("sits in Appearance with a real accessible name and says CLI applies a console palette", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, settingsProps()));
    expect(html).toContain('aria-label="Interface style: Refined"');
    expect(html).toContain("Interface style");
    // The mockup's words (owner, 2026-11-15); the menu's Bare CLI row still
    // says it switches to a console palette.
    expect(html).toContain("Refined is the standard look.");
    expect(html).toContain("Bare CLI is a flat, all-monospace console in Carbon.");
    // Keyboard reachable: the trigger is a real button inside a labelled group,
    // exactly like the theme picker beside it.
    expect(html)
      .toMatch(/<button[^>]*aria-label="Interface style: Refined"[^>]*aria-haspopup="listbox"/u);
    expect(html)
      .toMatch(/aria-labelledby="[^"]+-chrome"[^>]*aria-describedby="[^"]+-chrome-description"/u);
    // The chosen option explains itself on the trigger too, so the tooltip and
    // the menu row say the same thing.
    expect(html).toContain('title="Refined — Rounded corners, soft shadows and the sans-serif ramp."');
  });

  it("reflects the stored preference rather than a local default", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, settingsProps({
      preferences: { ...DEFAULT_APP_PREFERENCES, uiChrome: "cli" },
    })));
    expect(html).toContain('aria-label="Interface style: Bare CLI"');
    expect(html).not.toContain('aria-label="Interface style: Refined"');
    expect(html)
      .toContain('title="Bare CLI — Flat terminal console: monospace, square corners, no shadows or blur. Switches to a console palette."');
  });

  it("survives a corrupt preference object without losing the row", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage, settingsProps({
      preferences: {
        ...DEFAULT_APP_PREFERENCES, uiChrome: "terminal",
      } as unknown as AppPreferences,
    })));
    expect(html).toContain('aria-label="Interface style: Refined"');
  });
});

describe("the workspace surface", () => {
  it("contains its layout without a backdrop filter of its own", () => {
    // blur(var(--material-blur)) resolved to blur(0px) on this rule: it drew
    // nothing and still made the whole work area a render surface of its own
    // (about 90 MB of GPU-process footprint with four panes, measured 5 Oct).
    const surface = RULES.filter(rule => rule.selector === ".workspace-surface");
    expect(surface).toHaveLength(1);
    expect(surface[0]?.declarations.get("contain")).toBe("layout");
    expect(RULES.filter(rule => selectorParts(rule.selector)
      .some(part => part.startsWith(".workspace-surface"))
      && rule.declarations.has("backdrop-filter")).map(rule => rule.selector)).toEqual([]);
  });
});
