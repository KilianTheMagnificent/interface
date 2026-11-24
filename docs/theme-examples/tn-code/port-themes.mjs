#!/usr/bin/env node
/* Port TN Code's themes to Interface theme files.

       node port-themes.mjs [path to the TN Code checkout]

   TN Code (the owner's T3 Code fork, `~/.t3/projects/t3-apple`) keeps its
   palettes as sixty colour roles each and draws Orbit's and Glyph's moving
   patterns with fragment shaders. This reads them from that checkout and
   writes, beside this script, one Interface theme file per palette and
   appearance, and the shaders under shaders/. Run it again after TN Code's
   themes change; electron/src/renderer/theme-look.test.ts holds the files to
   what an Interface theme must be. Nothing is written into the checkout. */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const checkout = process.argv[2] ?? join(homedir(), ".t3", "projects", "t3-apple");

/** A TypeScript module of plain data and functions; Node (23.6 and later) reads it with its types stripped. */
const load = file => import(pathToFileURL(join(checkout, file)).href);
const json = file => JSON.parse(readFileSync(join(checkout, file), "utf8"));

/** `oklch(L C H)` as the nearest sRGB hex; a hex colour as it is. */
function hex(colour) {
  const match = /^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)$/u.exec(colour);
  if (match === null) {
    if (!/^#[0-9a-f]{6}$/iu.test(colour)) throw new Error(`a colour this does not read: ${colour}`);
    return colour.toLowerCase();
  }
  const [lightness, chroma, hue] = match.slice(1).map(Number);
  const a = chroma * Math.cos(hue * Math.PI / 180), b = chroma * Math.sin(hue * Math.PI / 180);
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return `#${linear.map(value => {
    const encoded = value <= 0.0031308 ? 12.92 * value : 1.055 * Math.max(value, 0) ** (1 / 2.4) - 0.055;
    return Math.round(Math.min(Math.max(encoded, 0), 1) * 255).toString(16).padStart(2, "0");
  }).join("")}`;
}

/* Interface works every other colour out from eight keys, so a palette's
   sixty roles come down to the ones that name the same thing. */
const KEYS = [
  ["--theme-background", "canvas"],
  ["--theme-panel", "sidebar"],
  ["--theme-raised", "surfaceRaised"],
  ["--theme-text", "text"],
  ["--theme-accent", "accent"],
  ["--theme-selection", "focus"],
  ["--theme-screen", "terminalBackground"],
  ["--theme-screen-text", "terminalForeground"],
];
const MEANING = [["--theme-danger", "error"], ["--theme-warning", "warning"]];
const TOKENS = [["--border", "border"], ["--accent-fg", "accentForeground"]];

const lines = (pairs, colours) => pairs.map(([token, role]) => `  ${token}: ${hex(colours[role])};`).join("\n");

/** WCAG contrast between two hex colours. */
function contrast(first, second) {
  const light = colour => {
    const [r, g, b] = [1, 3, 5].map(at => parseInt(colour.slice(at, at + 2), 16) / 255)
      .map(value => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const [high, low] = [light(first), light(second)].sort((a, b) => b - a);
  return (high + 0.05) / (low + 0.05);
}

/* Interface writes links and accent-coloured words in the accent itself. Some
   palettes (T3 Chat's dark side) keep the accent for fills and it is too dark
   to read as text, so those get the palette's nearest readable accent. */
function accentText(colours) {
  const canvas = hex(colours.canvas);
  if (contrast(hex(colours.accent), canvas) >= 4.5) return "";
  const role = ["focus", "updateForeground", "text"].find(name => contrast(hex(colours[name]), canvas) >= 4.5);
  return `\n  --accent-text: ${hex(colours[role])};`;
}

/** How each shader look sits behind Interface's glass: how far it shows through panels and screens. */
const LOOKS = {
  orbit: { shader: "orbit", glass: "46%", screen: "30%", says: "Orbit's sunrise horizon, an ordered dither that drifts and ripples" },
  glyph: { shader: "glyph", glass: "86%", screen: "80%", says: "Glyph's lattice of dots, brightening in drifting bands" },
};

function shaderLines(name, scene) {
  const source = readFileSync(join(here, "shaders", `${name}.frag`));
  return [
    `  --theme-backdrop: ${"BACKDROP"};`,
    `  --theme-shader: url("data:x-shader/x-fragment;base64,${source.toString("base64")}");`,
    `  --theme-shader-scale: ${scene.pixelScale};`,
    `  --theme-shader-rate: ${scene.framesPerSecond};`,
    `  --theme-shader-start: ${scene.startTime};`,
  ].join("\n");
}

function themeFile({ name, from, kind, colours, look, scene }) {
  const head = `/* ${name}: ${from}${look ? `, over ${look.says}` : ""}.

   Ported from TN Code by docs/theme-examples/tn-code/port-themes.mjs; change
   it there, not here. Copy it to ~/.config/interface/themes/ and pick it in
   Settings › Appearance. The format is in docs/THEMES.md. */`;
  const body = [
    `  --theme-kind: ${kind};`,
    "",
    "  /* Key colours: everything else is worked out from these. */",
    lines(KEYS, colours),
    "",
    "  /* Meaning colours, and the few tokens TN Code names outright. */",
    lines(MEANING, colours),
    lines(TOKENS, colours) + accentText(colours),
  ];
  if (look) {
    body.push("", "  /* The backdrop: the pattern is a fragment shader (shaders/" + look.shader + ".frag) drawn small",
      "     and stretched, behind panels and screens that let it through. */",
      shaderLines(look.shader, scene).replace("BACKDROP", hex(colours.canvas)),
      `  --theme-glass: ${look.glass};`, `  --theme-glass-screen: ${look.screen};`);
  }
  return `${head}\n:root {\n${body.join("\n")}\n}\n`;
}

const palettes = await load("packages/shared/src/themePalettes.ts");
const scenes = await load("apps/web/src/fork/features/theme-backdrops/scenes.ts");
const sceneOf = { orbit: scenes.backdropSceneFor("orbit", true), glyph: scenes.backdropSceneFor("glyph", true) };

mkdirSync(join(here, "shaders"), { recursive: true });
const shaders = {
  orbit: sceneOf.orbit, glyph: sceneOf.glyph,
  "picture-dither": scenes.imageBackdropScene("dither"), "picture-dots": scenes.imageBackdropScene("dots"),
};
for (const [name, scene] of Object.entries(shaders)) {
  writeFileSync(join(here, "shaders", `${name}.frag`), scene.fragmentSource);
}

/* Every theme TN Code's picker offers: its own four, T3 Code's two defaults
   and the five built-in palettes. A palette with a second appearance becomes
   two files, since an Interface theme file is light or dark. */
const fork = file => {
  const theme = json(`apps/web/src/fork/${file}`);
  return { label: theme.name, appearance: theme.appearance, colors: theme.colors, variants: theme.variants };
};
const sources = [
  // TN Code draws Orbit's horizon on its dark side; its light side stays plain.
  { ...fork("themes/tn-code-theme.json"), looks: { dark: "orbit" }, from: "TN Code's own theme" },
  { ...fork("themes/orbit-theme.json"), looks: { dark: "orbit" }, from: "TN Code's Orbit theme" },
  { ...fork("themes/glyph-theme.json"), looks: { dark: "glyph" }, from: "TN Code's Glyph theme" },
  { ...fork("apple/apple-theme.json"), from: "TN Code's Apple theme" },
  { label: "T3 Code", appearance: "dark", colors: palettes.T3_CODE_DARK_THEME_COLORS,
    variants: { light: palettes.T3_CODE_LIGHT_THEME_COLORS }, from: "T3 Code's default colours" },
  ...palettes.BUILT_IN_THEMES.map(theme => ({ ...theme, from: `T3 Code's ${theme.label} palette` })),
];

const written = [];
for (const source of sources) {
  const sides = [[source.appearance, source.colors], ...Object.entries(source.variants ?? {})];
  for (const [kind, colours] of sides) {
    const second = kind !== source.appearance;
    const name = second ? `${source.label} ${kind === "light" ? "Light" : "Dark"}` : source.label;
    const lookId = source.looks?.[kind];
    writeFileSync(join(here, `${name}.css`), themeFile({
      name, kind, colours, from: second ? `${source.from}, ${kind} side` : source.from,
      look: lookId ? LOOKS[lookId] : undefined, scene: lookId ? sceneOf[lookId] : undefined,
    }));
    written.push(name);
  }
}
console.log(`${written.length} themes: ${written.join(", ")}`);
console.log(`${Object.keys(shaders).length} shaders: ${Object.keys(shaders).join(", ")}`);
