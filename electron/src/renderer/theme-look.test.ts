import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { normalizeAppPreferences } from "./settings";
import {
  BACKDROP_BATTERY_FRAMES_PER_SECOND, BACKDROP_BATTERY_IDLE_MS, BACKDROP_FRAMES_PER_SECOND, BACKDROP_LONGEST_STEP_MS,
  FROST_BATTERY_REFRESH_MS, FROST_REFRESH_MS, THEME_BACKDROP_LAYERS, backdropFrameRate, backdropMoves, cleanScene,
  drawsBlur, frostRefreshDue, pairFrostAnimations, sceneSvgSource, shaderPicture,
  shaderSettings, shaderSource, stepBackdrop, themeDrawsBackdrop, type SceneNode,
} from "./theme-look";
import {
  SHADER_DEFAULT_RATE, SHADER_DEFAULT_SCALE, SHADER_MAX_PIXELS, sameShader, shaderCanvasSize, shaderDue, shaderNumber,
} from "./theme-shader";

/* Looks (owner, 2026-11-15): a theme file that sets a backdrop paints it under
   the whole window, panes, threads and the sidebar turn to glass over it, and
   the backdrop may move. These tests cover the decisions theme-look.ts makes
   and the rules theme-look.css must keep; the hands-free app covers the
   pixels. */

const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");
const withoutComments = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//gu, "");
const lookCss = withoutComments(read("./theme-look.css"));
const themeCss = withoutComments(read("./theme.css"));

const styles = (values: Record<string, string>) => ({
  getPropertyValue: (name: string): string => values[name] ?? "",
});

describe("a theme file's backdrop", () => {
  it("is drawn when the file sets --theme-backdrop or --theme-scene, and not otherwise", () => {
    expect(themeDrawsBackdrop(styles({}))).toBe(false);
    expect(themeDrawsBackdrop(styles({ "--theme-backdrop": " none ", "--theme-scene": "none" }))).toBe(false);
    expect(themeDrawsBackdrop(styles({ "--theme-backdrop": "linear-gradient(#000, #123)" }))).toBe(true);
    expect(themeDrawsBackdrop(styles({ "--theme-backdrop": "none", "--theme-scene": 'url("data:image/png;base64,AAAA")' }))).toBe(true);
  });

  it("turns panes, threads and the sidebar to glass only while a backdrop is painted", () => {
    // Every rule that changes a surface is scoped to the look, so a built-in
    // palette and a theme file without a backdrop paint exactly as before.
    const surfaceRules = [...lookCss.matchAll(/(^|\})\s*([^{}@]+)\{[^{}]*\bbackground(?:-color)?\s*:/gu)]
      .map(match => match[2]!.trim())
      .filter(selector => !selector.includes(".theme-backdrop"));
    expect(surfaceRules.length).toBeGreaterThan(5);
    for (const selector of surfaceRules) {
      for (const part of selector.split(",")) {
        expect(part.trim(), part).toMatch(/^:root\[data-theme-look="backdrop"\]/u);
      }
    }
    for (const surface of [".sidebar", ".terminal-pane", ".terminal-pane-head", ".workspace-tile",
      ".workspace-tile-head", ".workspace-hero", ".workspace-composer-panel", ".terminal-launcher"]) {
      expect(lookCss, surface).toMatch(new RegExp(
        `:root\\[data-theme-look="backdrop"\\] ${surface.replace(".", "\\.")}[ ,{]`, "u"));
    }
    // Terminals drop their own ground so the backdrop shows through the screen.
    expect(lookCss).toMatch(/:root\[data-theme-look="backdrop"\] \{[^}]*--terminal-glass-active: 1;/u);
  });

  it("fades every pane but the selected one, the way Rex does, and never a pane that is waiting", () => {
    // Owner, 2026-11-15: the selected pane was lost against a moving backdrop.
    // Rex's inactive panes sit at 71% of the active one's strength.
    const unselected = ':root[data-theme-look="backdrop"] .terminal-pane:not(.is-focused)'
      + ":not(:has(> .terminal-pane-head .terminal-tab-status.is-waiting))";
    const rule = lookCss.slice(lookCss.indexOf(`${unselected} > .terminal-pane-head > *,`));
    expect(rule.startsWith(`${unselected} > .terminal-pane-head > *,\n${unselected} > .terminal-pane-body {`)).toBe(true);
    expect(rule.slice(0, rule.indexOf("}"))).toMatch(/\{\s*opacity: 0\.71;\s*$/u);
    // What is in the head and the screen fades; the frame itself never does,
    // or its glass would let more of the picture through.
    const fading = [...lookCss.matchAll(/(^|\})\s*([^{}@]+)\{[^{}]*\bopacity\s*:/gu)].map(match => match[2]!.trim());
    expect(fading).toEqual([`${unselected} > .terminal-pane-head > *,\n${unselected} > .terminal-pane-body`]);
    // Only over a backdrop: a built-in theme's unselected panes are untouched.
    for (const sheet of ["./terminal.css", "./translucency.css"]) {
      expect(withoutComments(read(sheet)), sheet).not.toContain(".terminal-pane:not(.is-focused)");
    }
    // The change of selection crossfades, as Rex's does.
    expect(lookCss).toMatch(
      /:root\[data-theme-look="backdrop"\] \.terminal-pane-head > \*,\s*:root\[data-theme-look="backdrop"\] \.terminal-pane-body \{\s*transition: opacity var\(--motion-base\) var\(--ease-standard\);/u);
  });

  it("keeps its tokens in the palette layer, so a theme file's plain :root rule wins", () => {
    const layered = [...lookCss.matchAll(/@layer interface-palette \{([\s\S]*?)\n\}/gu)].map(match => match[1]!).join("\n");
    for (const token of ["--theme-backdrop", "--theme-scene", "--theme-backdrop-veil", "--theme-backdrop-blur",
      "--theme-glass", "--theme-glass-screen", "--theme-glass-blur"]) {
      expect(layered, token).toMatch(new RegExp(`${token}:`, "u"));
    }
    expect(layered).not.toContain("!important");
  });

  it("goes solid under macOS Reduce transparency", () => {
    const reduced = /@media \(prefers-reduced-transparency: reduce\) \{([\s\S]*?)\n\}/u.exec(lookCss)?.[1] ?? "";
    expect(reduced).toContain("--terminal-glass-active: 0;");
    expect(reduced).toContain("--look-panel-fill: 100%;");
    expect(reduced).toContain("--look-screen-fill: 100%;");
  });
});

describe("a moving backdrop", () => {
  it("runs only while Interface is in front, motion is allowed and Settings leaves it on", () => {
    const base = { windowActive: true, reducedMotion: false, preference: "on" as const };
    expect(backdropMoves(base)).toBe(true);
    expect(backdropMoves({ ...base, windowActive: false })).toBe(false);
    expect(backdropMoves({ ...base, reducedMotion: true })).toBe(false);
    expect(backdropMoves({ ...base, preference: "off" })).toBe(false);
  });

  it("is held in CSS by a rule a theme file cannot outrank, and stepped from here", () => {
    expect(lookCss).toMatch(
      /\.theme-backdrop,\s*\.theme-backdrop \*,[^{]*\{ animation-play-state: paused !important; \}/u);
    expect(read("./theme-look.ts")).toContain("*, *::before, *::after { animation-play-state: paused !important; }");
  });

  it("is drawn thirty times a second: each held animation moves on by the time that passed", () => {
    expect(BACKDROP_FRAMES_PER_SECOND).toBe(30);
    const held = { playState: "paused" as const, currentTime: 1000 as CSSNumberish | null };
    const fresh = { playState: "paused" as const, currentTime: null as CSSNumberish | null };
    const running = { playState: "running" as const, currentTime: 500 as CSSNumberish | null };
    expect(stepBackdrop([held, fresh, running], 33)).toBe(33);
    expect([held.currentTime, fresh.currentTime, running.currentTime]).toEqual([1033, 33, 500]);
    // A timer that stalled (sleep, a busy main thread) never lurches the scene.
    expect(stepBackdrop([held], 5000)).toBe(BACKDROP_LONGEST_STEP_MS);
    expect(held.currentTime).toBe(1033 + BACKDROP_LONGEST_STEP_MS);
    expect(stepBackdrop([held], -5)).toBe(0);
    expect(held.currentTime).toBe(1033 + BACKDROP_LONGEST_STEP_MS);
  });

  it("keeps looping where other loops rest after three seconds", () => {
    expect(read("./loop-rest.ts")).toContain(
      'if (event.target instanceof Element && event.target.closest(".theme-backdrop") !== null) return;');
  });

  /* Battery (owner, 2026-11-15: "optimize this new feature as best as you can
     to reduce its battery life consumption"). */
  it("is drawn half as often on the battery, and rests there after a minute without input", () => {
    const base = { windowActive: true, reducedMotion: false, preference: "on" as const };
    expect(backdropFrameRate(base)).toBe(BACKDROP_FRAMES_PER_SECOND);
    expect(backdropFrameRate({ ...base, onBattery: false, idleMs: 3_600_000 })).toBe(BACKDROP_FRAMES_PER_SECOND);
    expect(BACKDROP_BATTERY_FRAMES_PER_SECOND).toBe(15);
    expect(backdropFrameRate({ ...base, onBattery: true })).toBe(BACKDROP_BATTERY_FRAMES_PER_SECOND);
    expect(backdropFrameRate({ ...base, onBattery: true, idleMs: BACKDROP_BATTERY_IDLE_MS - 1 }))
      .toBe(BACKDROP_BATTERY_FRAMES_PER_SECOND);
    expect(BACKDROP_BATTERY_IDLE_MS).toBe(60_000);
    expect(backdropFrameRate({ ...base, onBattery: true, idleMs: BACKDROP_BATTERY_IDLE_MS })).toBe(0);
    expect(backdropMoves({ ...base, onBattery: true, idleMs: BACKDROP_BATTERY_IDLE_MS })).toBe(false);
    // Every other rest still holds on the battery.
    expect(backdropFrameRate({ ...base, onBattery: true, windowActive: false })).toBe(0);
    expect(backdropFrameRate({ ...base, onBattery: true, reducedMotion: true })).toBe(0);
    // A step never outruns the longest one a frame may take.
    expect(1000 / BACKDROP_BATTERY_FRAMES_PER_SECOND).toBeLessThan(BACKDROP_LONGEST_STEP_MS);
  });

  it("stays still on the battery when Settings says so, and moves on the power adapter", () => {
    const base = { windowActive: true, reducedMotion: false, preference: "plugged" as const };
    expect(backdropFrameRate({ ...base, onBattery: true })).toBe(0);
    expect(backdropFrameRate({ ...base, onBattery: false })).toBe(BACKDROP_FRAMES_PER_SECOND);
    expect(backdropFrameRate(base)).toBe(BACKDROP_FRAMES_PER_SECOND);
    expect(normalizeAppPreferences({ themeMotion: "plugged" }).themeMotion).toBe("plugged");
  });

  it("steps inside the display's own frame, so a window that is not drawn steps nothing", () => {
    const source = read("./theme-look.ts");
    expect(source).toContain("stepRequest = window.requestAnimationFrame(stepFrame);");
    expect(source).not.toContain("setInterval(");
  });

  it("writes a blur into the page only when the theme file asks for one", () => {
    for (const none of ["", " ", "0", "0px", " 0px ", "0.0px", ".0rem", "+0px", "-0px", "0%"]) {
      expect(drawsBlur(none), JSON.stringify(none)).toBe(false);
    }
    for (const some of ["1px", "0.5px", "12px", ".2rem", "calc(0px)", "10"]) {
      expect(drawsBlur(some), some).toBe(true);
    }
    // blur(0px) is still composited as a filter: the art and every panel are
    // blurred only under the attributes theme-look.ts sets from drawsBlur.
    // The one blur always written is the sidebar's frost (owner, 2026-11-16),
    // on a still copy of the art rather than on anything that moves.
    expect(lookCss).toMatch(/\.theme-backdrop\[data-blur\] \.theme-backdrop-art \{[^}]*filter: blur\(/u);
    const filters = [...lookCss.matchAll(/(^|\})\s*([^{}@]+)\{[^{}]*\b(?:backdrop-)?filter\s*:\s*blur\(/gu)]
      .map(match => match[2]!.trim());
    expect(filters.length).toBe(3);
    for (const selector of filters.flatMap(list => list.split(","))) {
      expect(selector.trim(), selector).toMatch(/\[data-blur\]|\[data-theme-frost\]|^\.theme-backdrop-frost-pane$/u);
    }
    // The sidebar's own material is a blur of zero as well.
    expect(lookCss).toMatch(
      /:root\[data-theme-look="backdrop"\]:not\(\[data-theme-frost\]\) \.sidebar \{[^}]*backdrop-filter: none;/u);
  });

  it("stores the Settings switch only once it is turned off", () => {
    expect(normalizeAppPreferences({})).not.toHaveProperty("themeMotion");
    expect(normalizeAppPreferences({ themeMotion: "on" })).not.toHaveProperty("themeMotion");
    expect(normalizeAppPreferences({ themeMotion: "off" }).themeMotion).toBe("off");
    expect(normalizeAppPreferences({ themeMotion: "sideways" })).not.toHaveProperty("themeMotion");
  });
});

const SCENE = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle r="2"/></svg>';

/* The sidebar over a live background (owner, 2026-11-16: it "looks a little
   weird"; shown solid, quiet glass, frosted and clear-under-the-list, "I quite
   like the frost glass look. Try to build it while being as battery friendly
   as possible"). Blurring what is behind the sidebar on every frame cost four
   points of a core in the graphics process (hands-free app, Kiln, two panes:
   about 6% to about 10%); a still copy that catches up twice a second costs
   what the plain glass did. */
describe("the sidebar's frost", () => {
  const rule = (selector: string): string => {
    const at = lookCss.indexOf(`${selector} {`);
    return at < 0 ? "" : lookCss.slice(at, lookCss.indexOf("}", at) + 1);
  };

  it("frosts the sidebar from a still copy of the picture, not by blurring behind it on every frame", () => {
    // The copy is blurred where it sits, under the sidebar's own box.
    expect(rule(".theme-backdrop-frost-pane")).toContain("filter: blur(26px) saturate(1.25);");
    const frost = rule(".theme-backdrop-frost");
    for (const side of ["left: anchor(--interface-sidebar left, 0px);", "top: anchor(--interface-sidebar top, 0px);",
      "width: anchor-size(--interface-sidebar width, 0px);", "height: anchor-size(--interface-sidebar height, 0px);"]) {
      expect(frost, side).toContain(side);
    }
    expect(lookCss).toContain(
      ':root[data-theme-look="backdrop"] .app-grid > .sidebar { anchor-name: --interface-sidebar; }');
    // The sidebar itself is tinted glass with no filter of its own, unless the theme file asks for one.
    expect(rule(':root[data-theme-look="backdrop"] .sidebar')).toBe(':root[data-theme-look="backdrop"] .sidebar {\n'
      + "  background: color-mix(in srgb, var(--material-chrome) var(--look-sidebar-fill), transparent);\n}");
    expect(lookCss).toContain("--look-sidebar-fill: max(56%, var(--look-panel-fill));");
    // Nothing in the copy runs by itself, and the window never measures the page for it.
    expect(lookCss).toMatch(/\.theme-backdrop-frost \*,\s*\.theme-backdrop-frost \*::before,\s*\.theme-backdrop-frost \*::after \{ animation-play-state: paused !important; \}/u);
    const source = read("./theme-look.ts");
    expect(source).not.toContain("getBoundingClientRect");
    expect(source).not.toContain("ResizeObserver");
  });

  it("catches up with the moving picture twice a second, and once a second on the battery", () => {
    expect(FROST_REFRESH_MS).toBe(500);
    expect(FROST_BATTERY_REFRESH_MS).toBe(1000);
    expect(frostRefreshDue(1000, 600, false)).toBe(false);
    expect(frostRefreshDue(1100, 600, false)).toBe(true);
    expect(frostRefreshDue(1100, 600, true)).toBe(false);
    expect(frostRefreshDue(1600, 600, true)).toBe(true);
    // Never drawn before: due at once.
    expect(frostRefreshDue(0, Number.NEGATIVE_INFINITY, true)).toBe(true);
  });

  it("moves each animation of the copy to the time of the one it copies", () => {
    const pairs = pairFrostAnimations(
      [["0.1||sway", "copy sway"], ["0.2||drift", "copy drift"], ["0.2||drift", "copy drift again"], ["9||gone", "copy only"]],
      [["0.2||drift", "live drift"], ["0.1||sway", "live sway"], ["0.2||drift", "live drift again"], ["4||new", "live only"]],
    );
    expect(pairs).toEqual([["copy sway", "live sway"], ["copy drift", "live drift"], ["copy drift again", "live drift again"]]);
  });

  it("is solid, with no frost drawn, where the sidebar comes back over the work or transparency is reduced", () => {
    expect(lookCss).toContain(
      ':root[data-theme-look="backdrop"] .app-grid.no-sidebar > .sidebar { background: var(--material-chrome); }');
    expect(lookCss).toContain(":root:has(.app-grid.no-sidebar) .theme-backdrop-frost { display: none; }");
    const narrow = /@media \(max-width: 840px\) \{([\s\S]*?)\n\}/u.exec(lookCss)?.[1] ?? "";
    expect(narrow).toContain(':root[data-theme-look="backdrop"] .sidebar { background: var(--material-chrome); }');
    expect(narrow).toContain(".theme-backdrop-frost { display: none; }");
    const reduced = /@media \(prefers-reduced-transparency: reduce\) \{([\s\S]*?)\n\}/u.exec(lookCss)?.[1] ?? "";
    expect(reduced).toContain("--look-sidebar-fill: 100%;");
    expect(reduced).toContain(".theme-backdrop-frost { display: none; }");
  });
});

describe("an SVG scene", () => {
  it("is read from a base64 or percent-encoded data URL", () => {
    const base64 = Buffer.from(SCENE, "utf8").toString("base64");
    expect(sceneSvgSource(`url("data:image/svg+xml;base64,${base64}")`)).toBe(SCENE);
    expect(sceneSvgSource(`url('data:image/svg+xml;charset=utf-8;base64,${base64}')`)).toBe(SCENE);
    expect(sceneSvgSource(`url("data:image/svg+xml,${encodeURIComponent(SCENE)}")`)).toBe(SCENE);
    // Non-ASCII survives the round trip.
    const snow = SCENE.replace("<circle", "<title>Schnee ❄</title><circle");
    expect(sceneSvgSource(`url("data:image/svg+xml;base64,${Buffer.from(snow, "utf8").toString("base64")}")`)).toBe(snow);
  });

  it("leaves any other picture to the CSS background", () => {
    expect(sceneSvgSource("none")).toBeNull();
    expect(sceneSvgSource('url("data:image/png;base64,iVBORw0KGgo=")')).toBeNull();
    expect(sceneSvgSource("linear-gradient(#000, #fff)")).toBeNull();
    expect(sceneSvgSource('url("data:image/svg+xml;base64,@@not-base64@@")')).toBeNull();
    expect(sceneSvgSource('url("data:image/svg+xml,%E0%A4%A")')).toBeNull();
  });

  /** A stand-in for a DOM element: what cleanScene reads and removes. */
  class Node implements SceneNode {
    parent: Node | undefined;
    readonly children: Node[] = [];
    constructor(
      readonly localName: string,
      readonly attributes: Record<string, string> = {},
      children: Node[] = [],
      readonly namespaceURI?: string | null,
    ) {
      for (const child of children) { child.parent = this; this.children.push(child); }
    }
    getAttributeNames(): string[] { return Object.keys(this.attributes); }
    getAttribute(name: string): string | null { return this.attributes[name] ?? null; }
    removeAttribute(name: string): void { delete this.attributes[name]; }
    remove(): void {
      const siblings = this.parent?.children;
      siblings?.splice(siblings.indexOf(this), 1);
    }
    outline(): string {
      const attributes = Object.entries(this.attributes).map(([name, value]) => ` ${name}=${value}`).join("");
      return `<${this.localName}${attributes}>${this.children.map(node => node.outline()).join("")}`;
    }
  }

  it("keeps what draws and drops scripts, foreign content, links and handlers", () => {
    const scene = new Node("svg", { viewBox: "0 0 10 10", onload: "steal()" }, [
      new Node("script"),
      new Node("style"),
      new Node("defs", {}, [new Node("linearGradient", { id: "sky" }, [new Node("stop", { offset: "0" })]),
        new Node("filter", { id: "soft" }, [new Node("feGaussianBlur", { stdDeviation: "2" })])]),
      new Node("g", { onclick: "steal()", ONMOUSEOVER: "steal()" }, [
        new Node("foreignObject", {}, [new Node("iframe")]),
        new Node("a", { href: "https://example.com" }, [new Node("circle")]),
        new Node("path", { d: "M0 0", fill: "url(#sky)" }, [new Node("animate", { attributeName: "d" })]),
        new Node("use", { href: "#shape" }),
        new Node("use", { "xlink:href": "https://example.com/sprite.svg#x" }),
        new Node("image", { href: "data:image/png;base64,AAAA" }),
        new Node("image", { href: "https://example.com/pixel.png" }),
        new Node("circle", { href: "data:image/png;base64,AAAA" }),
        new Node("set", { attributeName: "xlink:href", to: "https://example.com" }),
        new Node("animateTransform", { attributeName: "transform" }),
        new Node("animateMotion", {}, [new Node("mpath", { href: "#track" })]),
      ]),
    ]);
    cleanScene(scene);
    expect(scene.outline()).toBe(
      "<svg viewBox=0 0 10 10><style><defs><linearGradient id=sky><stop offset=0>"
      + "<filter id=soft><feGaussianBlur stdDeviation=2>"
      + "<g><path d=M0 0 fill=url(#sky)><animate attributeName=d><use href=#shape><use>"
      + "<image href=data:image/png;base64,AAAA><image><circle>"
      + "<animateTransform attributeName=transform><animateMotion><mpath href=#track>");
  });

  it("trims SMIL attributeName before rejecting href animations and drops non-SVG namespace elements", () => {
    const svgNs = "http://www.w3.org/2000/svg";
    const xhtmlNs = "http://www.w3.org/1999/xhtml";
    const scene = new Node("svg", { viewBox: "0 0 10 10" }, [
      new Node("use", { href: "#safe" }, [
        new Node("set", { attributeName: " href ", to: "https://evil.example/payload.svg#x" }, [], svgNs),
        new Node("animate", { attributeName: "\txlink:href\n", values: "https://evil.example" }, [], svgNs),
        new Node("animateTransform", { attributeName: " HREF\r" }, [], svgNs),
        new Node("animate", { attributeName: " opacity " }, [], svgNs),
      ], svgNs),
      new Node("style", {}, [], xhtmlNs),
      new Node("image", { src: "https://evil.example/beacon.png" }, [], xhtmlNs),
      new Node("circle", { r: "4" }, [], null),
      new Node("circle", { r: "5" }, [], svgNs),
    ], svgNs);
    cleanScene(scene);
    expect(scene.outline()).toBe("<svg viewBox=0 0 10 10><use href=#safe><animate attributeName= opacity ><circle r=5>");
  });
});

/* Shader backdrops (owner, 2026-11-15, "look at the current themes we have in
   TN Code and port those over to Interface"): TN Code's Orbit and Glyph draw
   their patterns with a fragment shader, and a theme file can carry one. */
describe("a shader backdrop", () => {
  const GLSL = "#version 300 es\nprecision highp float;\nout vec4 c;\nvoid main() { c = vec4(1.0); } // ✓\n";
  const shaderUrl = `url("data:x-shader/x-fragment;base64,${Buffer.from(GLSL, "utf8").toString("base64")}")`;

  it("is read from the theme file as a fragment shader's data URL", () => {
    expect(shaderSource(shaderUrl)).toBe(GLSL);
    expect(shaderSource(`url("data:text/plain,${encodeURIComponent(GLSL)}")`)).toBe(GLSL);
    expect(shaderSource("none")).toBeNull();
    expect(shaderSource('url("data:x-shader/x-fragment;base64,")')).toBeNull();
    // A picture or a scene is not a shader, and a shader is not a scene.
    expect(shaderSource('url("data:image/svg+xml;base64,PHN2Zy8+")')).toBeNull();
    expect(sceneSvgSource(shaderUrl)).toBeNull();
  });

  it("makes the theme a look on its own, with no backdrop or scene set", () => {
    expect(themeDrawsBackdrop(styles({ "--theme-shader": shaderUrl }))).toBe(true);
    expect(themeDrawsBackdrop(styles({ "--theme-shader": "none" }))).toBe(false);
  });

  it("takes its picture only as an embedded raster image", () => {
    expect(shaderPicture('url("data:image/webp;base64,AAAA")')).toBe("data:image/webp;base64,AAAA");
    expect(shaderPicture("url('data:image/png;base64,AAAA')")).toBe("data:image/png;base64,AAAA");
    expect(shaderPicture('url("https://example.com/photo.jpg")')).toBeNull();
    expect(shaderPicture('url("data:image/svg+xml;base64,PHN2Zy8+")')).toBeNull();
    expect(shaderPicture("none")).toBeNull();
  });

  it("is drawn small, at its own pace, from the second the file names", () => {
    expect(shaderSettings(styles({}))).toBeNull();
    expect(shaderSettings(styles({ "--theme-shader": shaderUrl }))).toEqual({
      source: GLSL, picture: null, scale: SHADER_DEFAULT_SCALE, rate: SHADER_DEFAULT_RATE, start: 0,
    });
    const orbit = shaderSettings(styles({
      "--theme-shader": shaderUrl, "--theme-shader-scale": " 0.4 ", "--theme-shader-rate": "20",
      "--theme-shader-start": "40", "--theme-shader-image": 'url("data:image/jpeg;base64,AAAA")',
    }))!;
    expect(orbit).toMatchObject({ scale: 0.4, rate: 20, start: 40, picture: "data:image/jpeg;base64,AAAA" });
    expect(sameShader(orbit, { ...orbit })).toBe(true);
    expect(sameShader(orbit, { ...orbit, start: 0 })).toBe(false);
    // Out of range or not a number: held to the range, or the default.
    const wild = shaderSettings(styles({
      "--theme-shader": shaderUrl, "--theme-shader-scale": "9", "--theme-shader-rate": "240",
      "--theme-shader-start": "soon",
    }))!;
    expect(wild).toMatchObject({ scale: 2, rate: BACKDROP_FRAMES_PER_SECOND, start: 0 });
    // Never slower than a step may be long, so its clock keeps time.
    expect(shaderSettings(styles({ "--theme-shader": shaderUrl, "--theme-shader-rate": "2" }))!.rate)
      .toBe(1000 / BACKDROP_LONGEST_STEP_MS);
    expect(shaderNumber("1e3", 7, 0, 10)).toBe(7);
    expect(shaderNumber("calc(1)", 7, 0, 10)).toBe(7);
  });

  it("never draws more than four megapixels, whatever the window", () => {
    expect(shaderCanvasSize(1440, 900, 0.4)).toEqual({ width: 576, height: 360 });
    expect(shaderCanvasSize(1440, 900, 1)).toEqual({ width: 1440, height: 900 });
    expect(shaderCanvasSize(0, 0, 1)).toEqual({ width: 1, height: 1 });
    const huge = shaderCanvasSize(5120, 2880, 1);
    expect(huge.width * huge.height).toBeLessThanOrEqual(SHADER_MAX_PIXELS);
    expect(huge.width / huge.height).toBeCloseTo(5120 / 2880, 2);
  });

  it("is drawn when its own rate says a frame is due, under the backdrop's stepper", () => {
    expect(shaderDue(1000, Number.NEGATIVE_INFINITY, 20)).toBe(true);
    expect(shaderDue(1050, 1000, 20)).toBe(true);
    expect(shaderDue(1047, 1000, 20)).toBe(true);
    expect(shaderDue(1033, 1000, 20)).toBe(false);
    expect(shaderDue(1033, 1000, 30)).toBe(true);
    // The stepper, with its rests and battery rules, is the only thing that moves it.
    const look = read("./theme-look.ts");
    expect(look).toContain("if (step > 0) shader?.advance(step, now);");
    expect(look).toContain("return animations.length > 0 || sceneTimed || shader !== undefined;");
    const drawing = read("./theme-shader.ts");
    expect(drawing).not.toMatch(/requestAnimationFrame|setInterval|setTimeout/u);
    expect(drawing).toContain('powerPreference: "low-power"');
  });

  it("is a canvas stretched over the window, and its tokens sit in the palette layer", () => {
    expect(lookCss).toMatch(/\.theme-backdrop-shader \{[^}]*image-rendering: pixelated;/u);
    const layered = [...lookCss.matchAll(/@layer interface-palette \{([\s\S]*?)\n\}/gu)].map(match => match[1]!).join("\n");
    for (const token of ["--theme-shader", "--theme-shader-image", "--theme-shader-scale", "--theme-shader-rate",
      "--theme-shader-start"]) {
      expect(layered, token).toMatch(new RegExp(`${token}:`, "u"));
    }
  });
});

describe("TN Code's themes, ported in docs/theme-examples/tn-code", () => {
  const folder = new URL("../../../docs/theme-examples/tn-code/", import.meta.url);
  const themes = readdirSync(folder).filter(name => name.endsWith(".css")).sort();
  const checker = fileURLToPath(new URL("../check-theme.mjs", folder));
  const KEYS = ["--theme-background", "--theme-panel", "--theme-raised", "--theme-text", "--theme-accent",
    "--theme-selection", "--theme-screen", "--theme-screen-text", "--theme-danger", "--theme-warning"];
  const luminance = (hex: string): number => {
    const [r, g, b] = [1, 3, 5].map(at => parseInt(hex.slice(at, at + 2), 16) / 255)
      .map(value => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
  };
  const contrast = (a: string, b: string): number => {
    const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (high! + 0.05) / (low! + 0.05);
  };

  it("are every theme TN Code's picker offers, one file per appearance", () => {
    expect(themes).toEqual([
      "Apple Dark.css", "Apple.css", "Ember Dark.css", "Ember.css", "Glyph.css", "Grove Dark.css", "Grove.css",
      "Iris Dark.css", "Iris.css", "Ocean Dark.css", "Ocean.css", "Orbit.css", "T3 Chat Dark.css", "T3 Chat.css",
      "T3 Code Light.css", "T3 Code.css", "TN Code Light.css", "TN Code.css",
    ]);
  });

  for (const name of themes) {
    it(`${name} is a theme file Interface can paint and read`, () => {
      const css = readFileSync(new URL(name, folder), "utf8");
      expect(Buffer.byteLength(css)).toBeLessThan(1_000_000);
      const kind = /--theme-kind: (light|dark);/u.exec(css)?.[1];
      expect(kind).toBe(/ Light\.css$/u.test(name) ? "light" : / Dark\.css$/u.test(name) ? "dark" : kind);
      const tokens = new Map([...css.matchAll(/(--[a-z-]+): (#[0-9a-f]{6});/gu)].map(match => [match[1]!, match[2]!]));
      for (const key of KEYS) expect(tokens.has(key), key).toBe(true);
      // Words read on the canvas, on panels, on cards and on the terminal screen.
      for (const ground of ["--theme-background", "--theme-panel", "--theme-raised"]) {
        expect(contrast(tokens.get("--theme-text")!, tokens.get(ground)!), ground).toBeGreaterThan(7);
      }
      expect(contrast(tokens.get("--theme-screen-text")!, tokens.get("--theme-screen")!)).toBeGreaterThan(7);
      // Accent-coloured words read too: the accent itself, or the stand-in the port wrote.
      const accentText = tokens.get("--accent-text") ?? tokens.get("--theme-accent")!;
      expect(contrast(accentText, tokens.get("--theme-background")!)).toBeGreaterThanOrEqual(4.5);
      const checked = spawnSync(process.execPath, [checker, fileURLToPath(new URL(name, folder))], { encoding: "utf8" });
      expect(checked.stdout).toContain("nothing is drawn again on every frame");
    });
  }

  it("draw Orbit's horizon behind TN Code and Orbit, and Glyph's dots behind Glyph", () => {
    const shaderOf = (name: string): string | null => {
      const value = /--theme-shader: (url\("[^"]+"\));/u.exec(readFileSync(new URL(name, folder), "utf8"))?.[1];
      return value === undefined ? null : shaderSource(value);
    };
    const source = (name: string): string => readFileSync(new URL(`shaders/${name}.frag`, folder), "utf8");
    expect(shaderOf("TN Code.css")).toBe(source("orbit"));
    expect(shaderOf("Orbit.css")).toBe(source("orbit"));
    expect(shaderOf("Glyph.css")).toBe(source("glyph"));
    for (const name of themes.filter(theme => !["TN Code.css", "Orbit.css", "Glyph.css"].includes(theme))) {
      expect(shaderOf(name), name).toBeNull();
    }
    // Each shader is WebGL2 GLSL that takes what theme-shader.ts gives it.
    for (const name of ["orbit", "glyph", "picture-dither", "picture-dots"]) {
      const glsl = source(name);
      expect(glsl.startsWith("#version 300 es"), name).toBe(true);
      expect(glsl).toContain("uniform vec2 u_resolution;");
      expect(glsl).toContain("uniform float u_time;");
      expect(glsl.includes("uniform sampler2D u_image;")).toBe(name.startsWith("picture-"));
    }
    // Drawn as TN Code draws them: Orbit at 0.4 of the window, twenty a second; Glyph one to one, fifteen.
    const settings = (name: string) => {
      const css = readFileSync(new URL(name, folder), "utf8");
      // The shader's value holds a `;` of its own, so it is read as a whole url().
      const value = (token: string): string => new RegExp(
        token === "--theme-shader" ? `${token}: (url\\("[^"]+"\\));` : `${token}: ([^;]+);`, "u").exec(css)?.[1] ?? "";
      return shaderSettings(styles(Object.fromEntries(["--theme-shader", "--theme-shader-scale", "--theme-shader-rate",
        "--theme-shader-start"].map(token => [token, value(token)]))))!;
    };
    expect(settings("Orbit.css")).toMatchObject({ scale: 0.4, rate: 20, start: 0 });
    expect(settings("Glyph.css")).toMatchObject({ scale: 1, rate: 15, start: 40 });
  });
});

describe("meaning colours", () => {
  /** `--token: value;` pairs of the first rule whose prelude ends with `selector`. */
  const declarations = (css: string, selector: string): Map<string, string> => {
    let at = css.indexOf(`${selector} {`);
    // Not the tail of a selector list that happens to end the same way.
    while (at > 0 && !selector.includes(",") && css.slice(0, at).trimEnd().endsWith(",")) {
      at = css.indexOf(`${selector} {`, at + 1);
    }
    if (at < 0) throw new Error(`no ${selector} block`);
    const body = css.slice(at + selector.length + 2, css.indexOf("\n  }", at) < 0 ? css.indexOf("\n}", at) : Math.min(
      ...[css.indexOf("\n  }", at), css.indexOf("\n}", at)].filter(index => index >= 0)));
    return new Map([...body.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/gu)].map(match => [match[1]!, match[2]!.trim()]));
  };
  const wash = (value: string): string => {
    const mix = /^color-mix\(in srgb, var\(--theme-[a-z]+, (#[0-9a-f]{6})\) ([\d.]+)%, transparent\)$/u.exec(value);
    if (mix === null) throw new Error(`not a wash: ${value}`);
    return `${mix[1]}${Math.round(Number(mix[2]) / 100 * 255).toString(16).padStart(2, "0")}`;
  };

  for (const kind of ["dark", "light"] as const) {
    it(`leave a ${kind} theme file that sets none exactly as it was`, () => {
      const shipped = declarations(themeCss, `:root[data-theme="custom-${kind}"]`);
      const look = declarations(lookCss, `:root[data-theme-file][data-theme="custom-${kind}"]`);
      expect([...look.keys()].sort()).toEqual(["--danger-fg", "--diff-added", "--diff-added-bg", "--diff-removed",
        "--diff-removed-bg", "--mode-accept", "--mode-auto", "--mode-danger", "--status-failed", "--status-waiting"]);
      for (const [token, value] of look) {
        const fallback = token.endsWith("-bg") ? wash(value)
          : /^var\(--theme-(?:danger|success|warning|pro), (#[0-9a-f]{6})\)$/u.exec(value)?.[1];
        expect(fallback, token).toBe(shipped.get(token));
      }
    });
  }

  it("hand the terminal its red, green, yellow and magenta, and nothing when unset", () => {
    const ansi = declarations(lookCss,
      ':root[data-theme-file][data-theme="custom-dark"],\n  :root[data-theme-file][data-theme="custom-light"]');
    expect(Object.fromEntries(ansi)).toEqual({
      "--ansi-red": "var(--theme-danger)", "--ansi-bright-red": "var(--theme-danger)",
      "--ansi-green": "var(--theme-success)", "--ansi-bright-green": "var(--theme-success)",
      "--ansi-yellow": "var(--theme-warning)", "--ansi-bright-yellow": "var(--theme-warning)",
      "--ansi-magenta": "var(--theme-pro)", "--ansi-bright-magenta": "var(--theme-pro)",
    });
  });
});

describe("the example looks in docs/theme-examples", () => {
  const folder = new URL("../../../docs/theme-examples/", import.meta.url);
  const examples = readdirSync(folder).filter(name => name.endsWith(".css"));

  it("ship at least a dark, a light and a layered one", () => {
    expect(examples.sort()).toEqual(["Aurora.css", "Neon Grid.css", "Paper Tide.css"]);
  });

  for (const name of examples) {
    it(`${name} is a theme file Interface can paint`, () => {
      const css = readFileSync(new URL(name, folder), "utf8");
      // Rust refuses a file over 1 MB.
      expect(Buffer.byteLength(css)).toBeLessThan(1_000_000);
      expect(css).toMatch(/--theme-kind: (?:light|dark);/u);
      expect(css).toMatch(/--theme-backdrop: (?!none)/u);
      const scene = /--theme-scene: (url\("[^"]+"\));/u.exec(css)?.[1];
      const svg = sceneSvgSource(scene ?? "");
      expect(svg?.startsWith("<svg ")).toBe(true);
      // The scene matches its editable source, and holds nothing the cleaner would remove.
      const source = name.replace(".css", "").toLowerCase().replace(/^paper /u, "").replace(" ", "-");
      expect(svg).toBe(readFileSync(new URL(`scenes/${source}.svg`, folder), "utf8"));
      // Embedded once: embed-scene.py replaces the whole value, leaving no earlier scene behind.
      expect(css.match(/base64,/gu)).toHaveLength(1);
      expect(svg).not.toMatch(/<script|<foreignObject|\son[a-z]+=|href="(?!#)/u);
      // A file's own layers stay within the ones the backdrop offers.
      for (const [, layer] of css.matchAll(/\[data-layer="(\d+)"\]/gu)) {
        expect(Number(layer)).toBeLessThanOrEqual(THEME_BACKDROP_LAYERS);
      }
    });
  }

  /* Moving or fading a finished picture costs almost nothing; what must be
     drawn again on every frame cost these examples half their battery use
     (measured 2026-11-15). check-theme.mjs names such things; the examples an
     agent copies from hold none. */
  const checker = fileURLToPath(new URL("check-theme.mjs", folder));
  const costs = (file: string) => spawnSync(process.execPath, [checker, file], { encoding: "utf8" });

  for (const name of examples) {
    it(`${name} draws nothing again on every frame`, () => {
      const checked = costs(fileURLToPath(new URL(name, folder)));
      expect(checked.stdout).toContain("nothing is drawn again on every frame");
      expect(checked.status).toBe(0);
    });
  }

  it("the checker names what a theme would draw again on every frame", () => {
    const scene = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><style>
      .s { animation: tw 4s infinite alternate; } @keyframes tw { from { opacity: .1; } to { opacity: 1; } }
      .halo { animation: pulse 6s infinite alternate; } @keyframes pulse { from { transform: scale(.9); } to { transform: scale(1.1); } }
      .bars { animation: slide 5s linear infinite; } @keyframes slide { to { transform: translateY(4px); } }
      .wing { animation: flap 1s infinite; } @keyframes flap { to { d: path("M0 0"); } }
      </style><defs><mask id="m"><rect class="bars" width="9" height="9"/></mask></defs>
      <g class="s"><circle r="1"/><circle cx="50" r="1"/></g><circle class="halo" r="9"/><path class="wing" d="M0 1"/>
      <rect width="5" height="5"><animate attributeName="x" values="0;5" dur="2s" repeatCount="indefinite"/></rect></svg>`;
    const theme = `:root { --theme-kind: dark; --theme-backdrop: #000; --theme-glass-blur: 8px;
        --theme-scene: url("data:image/svg+xml;base64,${Buffer.from(scene, "utf8").toString("base64")}"); }
      .theme-backdrop-layer[data-layer="1"] { filter: blur(30px); mix-blend-mode: screen; animation: drift 20s infinite; }
      .theme-backdrop-layer[data-layer="2"] { mask-image: linear-gradient(#000, transparent); }
      .theme-backdrop-layer[data-layer="2"]::before { animation: slide 2s linear infinite; }
      .theme-backdrop-layer[data-layer="3"] { animation: shift 9s infinite; }
      .theme-backdrop-layer[data-layer="4"] { animation: grow 9s infinite; will-change: transform; }
      @keyframes drift { to { transform: translateX(5%); } }
      @keyframes slide { to { transform: translateY(48px); } }
      @keyframes shift { to { background-position: 40px 0; } }
      @keyframes grow { from { transform: scale(1); } to { transform: scale(1.2); } }`;
    const file = join(mkdtempSync(join(tmpdir(), "theme-costs-")), "Costly.css");
    writeFileSync(file, theme);
    const checked = costs(file);
    expect(checked.status).toBe(1);
    for (const named of [
      '`.theme-backdrop-layer[data-layer="1"]` moves and has `filter`',
      '`.theme-backdrop-layer[data-layer="1"]` uses `mix-blend-mode`',
      '`.theme-backdrop-layer[data-layer="2"]` has `mask-image` over something that moves',
      '`.theme-backdrop-layer[data-layer="3"]` animates background-position',
      "`--theme-glass-blur: 8px` blurs every panel",
      '`<g class="s">` fades a group of 2 shapes',
      "`.halo` changes size",
      "`.bars` moves inside a <mask>",
      "`.wing` animates d",
      "SMIL",
    ]) expect(checked.stdout, named).toContain(named);
    // A backdrop layer that grows and shrinks is fine once its size is held.
    expect(checked.stdout).not.toContain('[data-layer="4"]');
    expect(checked.stdout).toContain("10 things cost battery");
  });
});

/* A theme file is any CSS, and a scene's own <style> is any CSS inside its
   shadow root, so `animation-play-state: running !important` with a more
   specific selector outranks theme-look.css's hold (review, 2026-11-15:
   measured in the hands-free app, such a scene kept spinning at 16 to 17% of a
   core while Interface was not the app in front). The module holds each
   backdrop animation by its own clock instead, which CSS no longer drives.
   The DOM here is the least applyThemeLook touches. */
describe("the stepper, driven through a page", () => {
  class FakeAnimation {
    playState: AnimationPlayState = "running";
    currentTime: number | null = 0;
    pause(): void { this.playState = "paused"; }
  }
  class FakeElement {
    className = "";
    readonly dataset: Record<string, string> = {};
    readonly children: FakeElement[] = [];
    readonly animations: FakeAnimation[] = [];
    readonly style = { setProperty: () => undefined, removeProperty: () => "" };
    parent: FakeElement | undefined;
    shadowRoot: { replaceChildren: () => void; append: () => void } | null = null;
    private readonly attributes = new Set<string>();
    get isConnected(): boolean { return this.parent !== undefined; }
    append(...nodes: FakeElement[]): void { for (const node of nodes) { node.parent = this; this.children.push(node); } }
    prepend(node: FakeElement): void { node.parent = this; this.children.unshift(node); }
    remove(): void { this.parent?.children.splice(this.parent.children.indexOf(this), 1); this.parent = undefined; }
    after(node: FakeElement): void {
      if (this.parent === undefined) return;
      node.parent = this.parent;
      this.parent.children.splice(this.parent.children.indexOf(this) + 1, 0, node);
    }
    replaceChildren(...nodes: FakeElement[]): void {
      for (const gone of this.children.splice(0)) gone.parent = undefined;
      this.append(...nodes);
    }
    // A clone carries the element and, deep, what is under it; its animations are its own.
    cloneNode(deep = false): FakeElement {
      const copy = new FakeElement();
      copy.className = this.className;
      Object.assign(copy.dataset, this.dataset);
      if (deep) copy.append(...this.children.map(child => child.cloneNode(true)));
      return copy;
    }
    setAttribute(name: string): void { this.attributes.add(name); }
    toggleAttribute(name: string, on: boolean): boolean { if (on) this.attributes.add(name); else this.attributes.delete(name); return on; }
    attachShadow() { this.shadowRoot = { replaceChildren: () => undefined, append: () => undefined }; return this.shadowRoot; }
    querySelector(selector: string): FakeElement | null {
      const name = /\.([\w-]+)$/u.exec(selector)?.[1];
      const direct = selector.startsWith(":scope >");
      for (const child of this.children) {
        if (child.className === name) return child;
        const deeper = direct ? null : child.querySelector(selector);
        if (deeper !== null) return deeper;
      }
      return null;
    }
    getAnimations(): FakeAnimation[] {
      return [...this.animations, ...this.children.flatMap(child => child.getAnimations())];
    }
    // A canvas where WebGL2 or the theme's shader cannot draw.
    hidden = false;
    readonly clientWidth = 800;
    readonly clientHeight = 600;
    getContext(): null { return null; }
    addEventListener(): void {}
    removeEventListener(): void {}
  }

  async function paintLook(windowActive: boolean, values: Record<string, string> = {}, running = true) {
    vi.resetModules();
    const root = new FakeElement();
    root.dataset.windowActive = String(windowActive);
    const body = new FakeElement();
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("document", {
      documentElement: root, body, createElement: () => new FakeElement(),
    });
    vi.stubGlobal("window", Object.assign(new EventTarget(), {
      matchMedia: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
      setTimeout: (task: () => void) => { task(); return 1; },
      clearTimeout: () => undefined,
      requestAnimationFrame: (frame: FrameRequestCallback) => { frames.push(frame); return frames.length; },
      cancelAnimationFrame: () => undefined,
    }));
    vi.stubGlobal("MutationObserver", class { observe(): void {} disconnect(): void {} });
    vi.stubGlobal("getComputedStyle", () => styles({ "--theme-backdrop": "linear-gradient(#000, #123)", ...values }));
    const look = await import("./theme-look");
    // The file's rule outranked the hold: its layer's loop runs by itself.
    const runaway = new FakeAnimation();
    look.applyThemeLook(false);
    expect(look.applyThemeLook(true)).toBe(true);
    const layer = body.querySelector(".theme-backdrop-layer")!;
    if (running) layer.animations.push(runaway);
    return { look, root, runaway, frames };
  }

  afterEach(() => { vi.unstubAllGlobals(); });

  it("is held all the same while Interface is behind another app", async () => {
    const { look, runaway } = await paintLook(false);
    look.applyThemeLook(true);
    expect(runaway.playState).toBe("paused");
  });

  it("is stepped like every other while the backdrop moves", async () => {
    const { look, root, runaway, frames } = await paintLook(true);
    look.applyThemeLook(true);
    expect(root.dataset.themeMotion).toBe("on");
    frames.shift()!(performance.now() + 33);
    expect(runaway.playState).toBe("paused");
    expect(runaway.currentTime).toBeGreaterThan(0);
  });

  /* The sidebar's frost (owner, 2026-11-16): the copy under the sidebar is
     the same art a second time. It is not stepped with the backdrop; it jumps
     to the backdrop's time when a refresh is due, so its blur is worked out a
     couple of times a second rather than on every frame. */
  it("leaves the sidebar's frosted copy still between refreshes, then brings it up to the picture", async () => {
    const { look, runaway, frames } = await paintLook(true);
    const body = (globalThis as unknown as { document: { body: FakeElement } }).document.body;
    const frost = body.querySelector(".theme-backdrop-frost")!;
    // The copy sits after the backdrop, holds the art again, and its layer runs the same loop.
    expect(body.children.map(child => child.className)).toEqual(["theme-backdrop", "theme-backdrop-frost"]);
    expect(frost.querySelector(".theme-backdrop-art")!.children.map(child => child.className))
      .toEqual(body.querySelector(".theme-backdrop-art")!.children.map(child => child.className));
    const copy = new FakeAnimation();
    frost.querySelector(".theme-backdrop-layer")!.animations.push(copy);
    look.applyThemeLook(true);
    expect(copy.playState).toBe("paused");
    const start = performance.now();
    frames.shift()!(start + 33);
    expect(runaway.currentTime).toBeGreaterThan(0);
    expect(copy.currentTime).toBe(0);
    frames.shift()!(start + 66);
    expect(copy.currentTime).toBe(0);
    frames.shift()!(start + 33 + FROST_REFRESH_MS);
    expect(copy.currentTime).toBe(runaway.currentTime);
    // Taken away with the backdrop.
    look.applyThemeLook(false);
    expect(body.children).toHaveLength(0);
  });

  /* Review, 2026-11-15: a theme whose shader does not compile (or where
     WebGL2 is missing) shows its plain backdrop, but the stepper kept waking
     about twenty times a second for it: 1.3% of a core in front against 0.3%
     still, in the hands-free app. */
  it("rests when its only motion is a shader that cannot draw", async () => {
    const glsl = Buffer.from("#version 300 es\nvoid main() { not glsl }\n", "utf8").toString("base64");
    const { look, root, frames } = await paintLook(true, {
      "--theme-shader": `url("data:x-shader/x-fragment;base64,${glsl}")`,
    }, false);
    look.applyThemeLook(true);
    for (let frame = 0; frame < 5 && frames.length > 0; frame += 1) frames.shift()!(performance.now());
    expect(root.dataset.themeMotion).toBe("rest");
    expect(frames).toHaveLength(0);
  });
});
