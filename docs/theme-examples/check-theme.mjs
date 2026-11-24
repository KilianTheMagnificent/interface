#!/usr/bin/env node
/* Check an Interface theme file for motion that drains the battery.

       node check-theme.mjs ~/.config/interface/themes/Mine.css

   A moving backdrop is drawn up to thirty times a second. Moving or fading a
   finished picture costs almost nothing; anything that must be drawn again on
   every frame costs several times more. This reads the theme (and the SVG
   scene inside it) and names what would be drawn again, with the fix. It
   prints nothing and exits 0 when the theme is clean, and exits 1 otherwise.
   The rules and the measurements behind them are in docs/THEMES.md. */
import { readFileSync } from "node:fs";

/** Properties a frame can change without anything being drawn again. */
const CHEAP = new Set(["transform", "opacity", "translate", "rotate"]);
const SHAPES = /<(?:path|rect|circle|ellipse|line|polygon|polyline|use|text|image)\b/gu;

const withoutComments = css => css.replace(/\/\*[\s\S]*?\*\//gu, "");

/** The SVG markup of the theme's --theme-scene, or null. */
function sceneOf(css) {
  const url = /--theme-scene\s*:\s*url\(\s*["']?(data:image\/svg\+xml[^"')]*)["']?\s*\)/u.exec(css)?.[1];
  if (url === undefined) return null;
  const comma = url.indexOf(",");
  const data = url.slice(comma + 1);
  try {
    return /;base64/iu.test(url.slice(0, comma)) ? Buffer.from(data, "base64").toString("utf8") : decodeURIComponent(data);
  } catch {
    return null;
  }
}

/** Top-level blocks of some CSS as [prelude, body] pairs. */
function blocks(css) {
  const found = [];
  let depth = 0, start = 0, prelude = "";
  for (let at = 0; at < css.length; at += 1) {
    const character = css[at];
    if (character === "{") {
      if (depth === 0) { prelude = css.slice(start, at).trim(); start = at + 1; }
      depth += 1;
    } else if (character === "}") {
      depth -= 1;
      if (depth === 0) { found.push([prelude, css.slice(start, at)]); start = at + 1; }
    } else if (character === ";" && depth === 0) {
      start = at + 1;
    }
  }
  return found;
}

const declarations = body => new Map([...body.matchAll(/(?:^|;)\s*([-a-z]+)\s*:\s*([^;]+)/giu)]
  .map(match => [match[1].toLowerCase(), match[2].trim()]));

/** Keyframes by name and declarations by single selector, through @media and the like. */
function read(css) {
  const keyframes = new Map(), rules = new Map();
  const walk = text => {
    for (const [prelude, body] of blocks(text)) {
      const frames = /^@(?:-webkit-)?keyframes\s+(\S+)/u.exec(prelude);
      if (frames !== null) {
        const properties = new Map();
        for (const [, frame] of blocks(body)) {
          for (const [name, value] of declarations(frame)) {
            if (!properties.has(name)) properties.set(name, new Set());
            properties.get(name).add(value);
          }
        }
        keyframes.set(frames[1], properties);
      } else if (prelude.startsWith("@")) {
        if (/^@(?:media|supports|layer|container)\b/u.test(prelude)) walk(body);
      } else {
        for (const selector of prelude.split(",").map(part => part.trim()).filter(Boolean)) {
          rules.set(selector, new Map([...(rules.get(selector) ?? []), ...declarations(body)]));
        }
      }
    }
  };
  walk(css);
  return { keyframes, rules };
}

/** The @keyframes names a rule runs. */
function animationsOf(style, keyframes) {
  const names = new Set();
  for (const property of ["animation", "animation-name"]) {
    for (const word of (style.get(property) ?? "").split(/[\s,]+/u)) if (keyframes.has(word)) names.add(word);
  }
  return [...names];
}

const set = value => value !== undefined && !/^(?:none|normal|auto|0(?:px)?)$/iu.test(value);
/** The sizes a set of transform values scales to, as text. */
const scales = values => new Set([...values].flatMap(value =>
  [...value.matchAll(/\b(?:scale(?:X|Y|3d)?|matrix(?:3d)?)\([^)]*\)/gu)].map(match => match[0].replace(/\s+/gu, ""))));

function check(css, where, inScene) {
  const findings = [];
  const { keyframes, rules } = read(css);
  const say = (subject, problem, fix) => findings.push(`${where}: ${subject} ${problem}\n    fix: ${fix}`);
  const moving = new Map();
  for (const [selector, style] of rules) {
    const names = animationsOf(style, keyframes);
    if (names.length > 0) moving.set(selector, names);
  }
  for (const [selector, names] of moving) {
    const style = rules.get(selector);
    for (const name of names) {
      const properties = keyframes.get(name);
      const costly = [...properties.keys()].filter(property => !CHEAP.has(property) && !property.startsWith("animation-"));
      if (costly.length > 0) {
        say(`\`${selector}\``, `animates ${costly.join(", ")} (@keyframes ${name}), which is drawn again on every frame.`,
          "move it with transform and fade it with opacity; nothing else is free.");
      }
      const sizes = new Set([...scales(properties.get("transform") ?? []), ...(properties.get("scale") ?? [])]);
      const held = !inScene && /\btransform\b/u.test(style.get("will-change") ?? "");
      if (sizes.size > 1 && !held) {
        say(`\`${selector}\``, `changes size (@keyframes ${name}), so it is drawn again at each size.`,
          inScene ? "in a scene, pulse with opacity or move with translate and rotate; grow and shrink a backdrop layer instead."
            : "add `will-change: transform` to the rule, or pulse with opacity.");
      }
    }
    for (const property of ["filter", "backdrop-filter", "-webkit-backdrop-filter"]) {
      if (set(style.get(property))) {
        say(`\`${selector}\``, `moves and has \`${property}\`, which is worked out again on every frame.`,
          "paint the picture and its filter on a still child (the layer's ::before) and animate the parent.");
      }
    }
  }
  for (const [selector, style] of rules) {
    if (set(style.get("mix-blend-mode"))) {
      say(`\`${selector}\``, "uses `mix-blend-mode`, which makes the window put the whole backdrop together again on every redraw.",
        "use plain see-through colours (an alpha in the colour, or opacity).");
    }
    const mask = ["mask", "mask-image", "-webkit-mask", "-webkit-mask-image", "clip-path"].find(property => set(style.get(property)));
    const inside = [...moving.keys()].some(other => other === selector || other.startsWith(`${selector}::`) || other.startsWith(`${selector} `));
    if (mask !== undefined && inside) {
      say(`\`${selector}\``, `has \`${mask}\` over something that moves, so the mask is worked out again on every frame.`,
        "cover the edge with a still gradient on top (::after) instead of masking it.");
    }
  }
  return { findings, moving, keyframes, rules };
}

function checkScene(svg) {
  const findings = [];
  const style = [...svg.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/gu)].map(match => match[1]).join("\n");
  const sheet = check(withoutComments(style), "scene", true);
  findings.push(...sheet.findings);
  const say = (subject, problem, fix) => findings.push(`scene: ${subject} ${problem}\n    fix: ${fix}`);
  if (/<(?:animate|animateTransform|animateMotion|set)\b/u.test(svg)) {
    say("SMIL (`<animate>`, `<animateTransform>`, `<animateMotion>`, `<set>`)", "draws its shape again on every frame.",
      "animate with CSS in the scene's <style>: @keyframes on transform and opacity.");
  }
  /** Classes whose rule runs an animation, and those among them that fade. */
  const classes = new Map();
  for (const [selector, names] of sheet.moving) {
    const name = /^\.([\w-]+)$/u.exec(selector)?.[1] ?? /\.([\w-]+)(?!.*\.)/u.exec(selector)?.[1];
    if (name === undefined) continue;
    const fades = names.some(keyframe => sheet.keyframes.get(keyframe).has("opacity"));
    classes.set(name, (classes.get(name) ?? false) || fades);
  }
  const classOf = tag => (/\bclass="([^"]*)"/u.exec(tag)?.[1] ?? "").split(/\s+/u);
  // A faded group of several shapes is composited on its own, at its full size, on every frame.
  for (const match of svg.matchAll(/<g\b([^>]*)>([\s\S]*?)<\/g>/gu)) {
    const fading = classOf(match[1]).find(name => classes.get(name) === true);
    const shapes = (match[2].match(SHAPES) ?? []).length;
    if (fading !== undefined && shapes > 1) {
      say(`\`<g class="${fading}">\``, `fades a group of ${shapes} shapes, which the window puts together separately on every frame.`,
        "draw the group as one <path> (several shapes in one `d`) and fade that.");
    }
  }
  for (const match of svg.matchAll(/<(mask|clipPath|pattern|filter)\b[^>]*>([\s\S]*?)<\/\1>/gu)) {
    const inside = [...match[2].matchAll(/<\w+\b[^>]*>/gu)].flatMap(tag => classOf(tag[0])).find(name => classes.has(name));
    if (inside !== undefined) {
      say(`\`.${inside}\``, `moves inside a <${match[1]}>, so everything it touches is drawn again on every frame.`,
        "keep what is inside a mask, clip, pattern or filter still.");
    }
  }
  for (const match of svg.matchAll(/<\w+\b[^>]*\bfilter="url\([^"]*"[^>]*>/gu)) {
    const name = classOf(match[0]).find(candidate => classes.has(candidate));
    if (name !== undefined) {
      say(`\`.${name}\``, "moves and has an SVG filter, which is worked out again on every frame.",
        "blur or glow a still shape, or use a soft gradient instead of a filter.");
    }
  }
  return findings;
}

/** Everything in a theme file that would be drawn again on every frame. */
export function themeCosts(source) {
  const css = withoutComments(source);
  const scene = sceneOf(css);
  // Embedded files (a scene, a shader, a picture) hold `;` and `{` of their own: read around them.
  const plain = css.replace(/url\(\s*(["'])data:[^"']*\1\s*\)/gu, "url(data:)");
  const findings = check(plain, "theme", false).findings;
  const scale = Number(/--theme-shader-scale\s*:\s*([\d.]+)\s*;/u.exec(plain)?.[1]);
  if (scale > 1) {
    findings.push(`theme: \`--theme-shader-scale: ${scale}\` draws the shader on more pixels than the window's own points.`
      + "\n    fix: 1 or less; a shader is drawn small and stretched (TN Code's Orbit uses 0.4).");
  }
  for (const token of ["--theme-glass-blur", "--theme-backdrop-blur"]) {
    const value = new RegExp(`${token}\\s*:\\s*([^;]+);`, "u").exec(css)?.[1]?.trim();
    if (set(value)) {
      findings.push(`theme: \`${token}: ${value}\` blurs ${token === "--theme-glass-blur" ? "every panel" : "the whole backdrop"} again on every redraw.`
        + "\n    fix: leave it at 0px over a moving backdrop; soften the picture itself instead.");
    }
  }
  if (scene !== null) findings.push(...checkScene(scene));
  // The same finding several times over (six groups of stars) is said once.
  const counts = new Map();
  for (const finding of findings) counts.set(finding, (counts.get(finding) ?? 0) + 1);
  return [...counts].map(([finding, count]) => count === 1 ? finding : finding.replace("\n", ` (${count} of these)\n`));
}

if (process.argv[1] !== undefined && import.meta.url.endsWith(process.argv[1].split("/").pop())) {
  const path = process.argv[2];
  if (path === undefined || path === "--help" || path === "-h") {
    console.error("usage: node check-theme.mjs <theme.css>\nNames what in a theme would be drawn again on every frame, and the fix.");
    process.exit(2);
  }
  const findings = themeCosts(readFileSync(path, "utf8"));
  if (findings.length === 0) {
    console.log(`${path}: nothing is drawn again on every frame.`);
  } else {
    console.log(`${path}: ${findings.length} thing${findings.length === 1 ? "" : "s"} cost battery:\n`);
    for (const finding of findings) console.log(`- ${finding}\n`);
    process.exit(1);
  }
}
