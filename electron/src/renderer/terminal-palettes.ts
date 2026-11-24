import type { ITheme } from "@xterm/xterm";
import type { ResolvedTheme } from "../shared/appearance";

/**
 * The 16 ANSI colours each theme hands its terminals (owner, 2026-09-28).
 * Before this every theme left them at xterm's defaults, so bright yellow,
 * green, cyan and white vanished on light screens and every palette's
 * terminal looked the same. Every colour except black clears 4.5:1 on its
 * theme's --terminal-content-bg (bright black, used for dim text, clears 3:1);
 * terminal-palettes.test.ts measures each against theme.css.
 */
export type TerminalAnsiPalette = Required<Pick<ITheme,
  | "black" | "red" | "green" | "yellow" | "blue" | "magenta" | "cyan" | "white"
  | "brightBlack" | "brightRed" | "brightGreen" | "brightYellow"
  | "brightBlue" | "brightMagenta" | "brightCyan" | "brightWhite">>;

/** Both Porcelain themes draw dark screens, so they share one palette. */
const PORCELAIN_SCREENS: TerminalAnsiPalette = {
  black: "#3a3c40", red: "#ef6b73", green: "#8cc98f", yellow: "#e6c07b",
  blue: "#7aa7ff", magenta: "#c9a0f0", cyan: "#6fc6c8", white: "#d5d7da",
  brightBlack: "#7c8087", brightRed: "#ff8a92", brightGreen: "#a6dba8", brightYellow: "#f2d493",
  brightBlue: "#9dbdff", brightMagenta: "#dbbaff", brightCyan: "#8ddadb", brightWhite: "#f2f3f5",
};

/** The clay hues, deepened until each clears 4.5:1 on the ivory screen. */
const CLAY_LIGHT: TerminalAnsiPalette = {
  black: "#141413", red: "#b62d44", green: "#546838", yellow: "#845917",
  blue: "#396496", magenta: "#754dac", cyan: "#2f6f6a", white: "#5e5b53",
  brightBlack: "#6f6c64", brightRed: "#c7394f", brightGreen: "#647a45", brightYellow: "#96661a",
  brightBlue: "#4977ab", brightMagenta: "#8a60c2", brightCyan: "#377f79", brightWhite: "#3d3d3a",
};

/** Anthropic's clay, blue, olive and kraft on slate. */
const CLAY_DARK: TerminalAnsiPalette = {
  black: "#3a3935", red: "#e56c76", green: "#819b5d", yellow: "#d4a27f",
  blue: "#6a9bcc", magenta: "#a57fda", cyan: "#6fb5ad", white: "#cfccc1",
  brightBlack: "#7d7a72", brightRed: "#f08a92", brightGreen: "#9cb878", brightYellow: "#e3b996",
  brightBlue: "#8ab4e0", brightMagenta: "#bd9ce8", brightCyan: "#8ecac2", brightWhite: "#faf9f5",
};

/** Ethan Schoonover's accents, darkened to 4.5:1 on base3; the whites are base01/base02 so they stay visible. */
const SOLARIZED_LIGHT: TerminalAnsiPalette = {
  black: "#073642", red: "#d72c2a", green: "#677900", yellow: "#946900",
  blue: "#0075bb", magenta: "#cc2e7c", cyan: "#007f77", white: "#586e75",
  brightBlack: "#657b83", brightRed: "#c6460e", brightGreen: "#677900", brightYellow: "#946900",
  brightBlue: "#0075bb", brightMagenta: "#6468ba", brightCyan: "#007f77", brightWhite: "#073642",
};

/** Ethan Schoonover's accents, the reds, blue and magenta lifted to 4.5:1 on base03. */
const SOLARIZED_DARK: TerminalAnsiPalette = {
  black: "#073642", red: "#f64d45", green: "#859900", yellow: "#b58900",
  blue: "#2c90d7", magenta: "#ec4f97", cyan: "#2aa198", white: "#eee8d5",
  brightBlack: "#5b7178", brightRed: "#e46232", brightGreen: "#a4b72a", brightYellow: "#d0a61a",
  brightBlue: "#5ea7e6", brightMagenta: "#8a8fdc", brightCyan: "#3cc2b6", brightWhite: "#fdf6e3",
};

/** Catppuccin Mocha's own terminal colours; bright black is overlay1 so dim text stays legible. */
const CATPPUCCIN_MOCHA: TerminalAnsiPalette = {
  black: "#45475a", red: "#f38ba8", green: "#a6e3a1", yellow: "#f9e2af",
  blue: "#89b4fa", magenta: "#f5c2e7", cyan: "#94e2d5", white: "#bac2de",
  brightBlack: "#7f849c", brightRed: "#f38ba8", brightGreen: "#a6e3a1", brightYellow: "#f9e2af",
  brightBlue: "#89b4fa", brightMagenta: "#f5c2e7", brightCyan: "#94e2d5", brightWhite: "#a6adc8",
};

/** The Grok Build CLI's console colours. */
const CARBON: TerminalAnsiPalette = {
  black: "#2e2e2e", red: "#ff7b6b", green: "#8aae8e", yellow: "#f2c96b",
  blue: "#4da6e0", magenta: "#b98ae6", cyan: "#6cc4c4", white: "#e8e2d6",
  brightBlack: "#7a756a", brightRed: "#ff9a8c", brightGreen: "#a6c9aa", brightYellow: "#f7d98f",
  brightBlue: "#7cc0ec", brightMagenta: "#cda7ef", brightCyan: "#8fd6d6", brightWhite: "#f5efe3",
};

/** Cream, rose, mint and gold on navy. */
const NAVY: TerminalAnsiPalette = {
  black: "#2a3a78", red: "#f0a8b8", green: "#8fd9a8", yellow: "#e8c07a",
  blue: "#9cc4ff", magenta: "#d8b4f0", cyan: "#8ad6e0", white: "#eddcc8",
  brightBlack: "#9aa0c8", brightRed: "#f7c3cf", brightGreen: "#aee6c1", brightYellow: "#f0d29c",
  brightBlue: "#bcd7ff", brightMagenta: "#e6cdf6", brightCyan: "#aee4eb", brightWhite: "#f4ebe0",
};

/** Klein blue and deep inks on a white screen; yellow is amber and white is a dark grey. */
const ICE: TerminalAnsiPalette = {
  black: "#1a1a1c", red: "#b4234d", green: "#1a6b3a", yellow: "#8a5a00",
  blue: "#1a3fd0", magenta: "#7a2f94", cyan: "#0e6e78", white: "#5a5b60",
  brightBlack: "#6e6e74", brightRed: "#d0314f", brightGreen: "#22804a", brightYellow: "#9e6900",
  brightBlue: "#2451f2", brightMagenta: "#9440b0", brightCyan: "#05808b", brightWhite: "#3d3e43",
};

export const TERMINAL_ANSI_PALETTES: Readonly<Record<ResolvedTheme, TerminalAnsiPalette>> = {
  light: PORCELAIN_SCREENS,
  dark: PORCELAIN_SCREENS,
  "anthropic-light": CLAY_LIGHT,
  "anthropic-dark": CLAY_DARK,
  "solarized-light": SOLARIZED_LIGHT,
  "solarized-dark": SOLARIZED_DARK,
  "catppuccin-mocha": CATPPUCCIN_MOCHA,
  "grok-build": CARBON,
  hermes: NAVY,
  "hermes-light": ICE,
};

const ANSI_KEYS = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "brightBlack", "brightRed", "brightGreen", "brightYellow",
  "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
] as const satisfies readonly (keyof TerminalAnsiPalette)[];

/** `brightBlack` → `--ansi-bright-black`: the property a theme file sets. */
export function ansiProperty(key: keyof TerminalAnsiPalette): string {
  return `--ansi-${key.replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`)}`;
}

function hexLuminance(value: string): number | null {
  const match = /^#([0-9a-f]{3}|[0-9a-f]{6})([0-9a-f]{2})?$/i.exec(value.trim());
  if (match === null) return null;
  const hex = match[1]!.length === 3 ? [...match[1]!].map(c => c + c).join("") : match[1]!;
  const channel = (offset: number): number => {
    const v = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}

/**
 * Whether the theme on `styles` draws light screens, built-in or a file's.
 * A hex screen colour decides light or dark;
 * otherwise the file's --terminal-color-scheme does.
 */
function isLightScreen(styles: Pick<CSSStyleDeclaration, "getPropertyValue">): boolean {
  const screen = hexLuminance(styles.getPropertyValue("--terminal-content-bg"));
  const lightScreen = screen === null
    ? styles.getPropertyValue("--terminal-color-scheme").trim() === "light" : screen > 0.4;
  return lightScreen;
}

/**
 * The contrast xterm holds text to on a light screen: AAA, the owner's pick
 * over AA on 2026-10-23. CLIs pick fixed colours made for dark terminals (a
 * status line's orange, Claude's yellow mode line), and on Clay Light's ivory
 * they fade to 1.6–3.4:1. xterm darkens a cell under this ratio as it paints,
 * asking half of it for dim text, so a theme switch fixes text already on
 * screen. Dark screens keep xterm's 1: their faint tiers are settled looks.
 */
export const LIGHT_SCREEN_MINIMUM_CONTRAST = 7;

/** The `minimumContrastRatio` for the screen the theme on `styles` draws. */
export function terminalMinimumContrast(styles: Pick<CSSStyleDeclaration, "getPropertyValue">): number {
  return isLightScreen(styles) ? LIGHT_SCREEN_MINIMUM_CONTRAST : 1;
}

/** A theme file's 16 colours: each `--ansi-*` it sets, the rest from the
 * built-in set made for its screen. */
function themeFilePalette(styles: Pick<CSSStyleDeclaration, "getPropertyValue">): TerminalAnsiPalette {
  const palette: TerminalAnsiPalette = { ...(isLightScreen(styles) ? ICE : PORCELAIN_SCREENS) };
  for (const key of ANSI_KEYS) {
    const value = styles.getPropertyValue(ansiProperty(key)).trim();
    if (value !== "") palette[key] = value;
  }
  return palette;
}

/** The palette for the theme on the document, or none for an unknown one.
 * A theme file's comes from its CSS, read off `styles`. */
export function terminalAnsiPalette(theme: string | undefined,
    styles?: Pick<CSSStyleDeclaration, "getPropertyValue">): Partial<TerminalAnsiPalette> {
  if ((theme === "custom-dark" || theme === "custom-light") && styles !== undefined) {
    return themeFilePalette(styles);
  }
  return theme !== undefined && Object.hasOwn(TERMINAL_ANSI_PALETTES, theme)
    ? TERMINAL_ANSI_PALETTES[theme as ResolvedTheme] : {};
}
