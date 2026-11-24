/**
 * Looks (owner, 2026-11-15): a theme file that sets `--theme-backdrop` or
 * `--theme-scene` paints one backdrop under the whole window, and panes,
 * threads and the sidebar become glass over it (theme-look.css), so the
 * picture runs unbroken behind every surface. The backdrop is plain elements a
 * file may style and animate with any CSS; an SVG scene is drawn as real SVG,
 * so its own animations can be held. docs/THEMES.md is the author's guide.
 *
 * Motion follows the battery rules the other loops do (window-activity.ts):
 * it rests while Interface is not the app in front, under macOS Reduce
 * motion, and when Settings turns moving backdrops off. Unlike other loops it
 * does not rest after three seconds (loop-rest.ts): moving is what a moving
 * backdrop is for, and it only runs in a theme the owner picked.
 *
 * A running CSS loop makes Chromium draw the whole window at the display's
 * refresh rate. Measured in the hands-free app on 2026-11-15, the Aurora
 * example cost 51% of a core that way and 18% drawn thirty times a second, so
 * the backdrop's animations are always paused in CSS and this module steps
 * them: every animation's clock moves forward by the time that passed, thirty
 * times a second, and the window is drawn only then.
 *
 * Battery (owner, 2026-11-15, "optimize this new feature as best as you can
 * to reduce its battery life consumption"). Measured the same way with two
 * terminal panes open, every drawn frame costs about the same whatever the
 * picture, so the saving is in drawing fewer of them and in not paying for
 * work nobody sees:
 * - on the battery the backdrop is drawn fifteen times a second, not thirty,
 *   and rests once nothing has been typed, clicked or scrolled for a minute;
 *   Settings can keep it still on the battery altogether;
 * - a step is taken inside the display's own frame, so a window that is not
 *   being drawn (covered, display asleep) steps nothing;
 * - a look with nothing to animate runs no timer at all;
 * - a blur of zero is left out: `filter: blur(0px)` on the art and
 *   `backdrop-filter: blur(0px)` on every panel still made Chromium composite
 *   each of them separately on every redraw, which cost 3 points of a core at
 *   rest and up to 7 while moving.
 *
 * The sidebar's frost (owner, 2026-11-16: over a live background the sidebar
 * "looks a little weird"; shown four treatments, "I quite like the frost
 * glass look. Try to build it while being as battery friendly as possible").
 * A `backdrop-filter` on the sidebar blurs what is behind it again on every
 * frame the backdrop moves: measured in the hands-free app over Kiln with two
 * panes, the graphics process went from about 6% of a core to about 10%. So
 * the sidebar is frosted from a second, still copy of the art instead: it sits
 * under the sidebar, is blurred there, and only catches up with the moving
 * picture twice a second (once a second on the battery), which under that
 * blur cannot be told from following it. Measured the same way it costs what
 * the plain glass sidebar did (about 6%). theme-look.css places the copy by
 * the sidebar's own box, so nothing here measures the page.
 */

import {
  SHADER_DEFAULT_RATE, SHADER_DEFAULT_SCALE, mountThemeShader, sameShader, shaderNumber,
  type MountedShader, type ShaderSettings,
} from "./theme-shader";

/** Empty layers above the scene, `data-layer="1"` …, for a file's own CSS. */
export const THEME_BACKDROP_LAYERS = 6;

const BACKDROP_PROPERTIES = ["--theme-backdrop", "--theme-scene", "--theme-shader"] as const;

type Styles = Pick<CSSStyleDeclaration, "getPropertyValue">;

/** Whether the theme on `styles` draws a backdrop: its file sets one of the
 * backdrop properties to something other than `none`. */
export function themeDrawsBackdrop(styles: Styles): boolean {
  return BACKDROP_PROPERTIES.some(name => {
    const value = styles.getPropertyValue(name).trim();
    return value !== "" && value !== "none";
  });
}

/** The address inside a CSS `url(…)` value, or nothing when it is not one. */
function urlOf(value: string): string | undefined {
  const match = /^url\(\s*(?:"([^"]*)"|'([^']*)'|([^"'()\s]*))\s*\)$/s.exec(value.trim());
  return match?.[1] ?? match?.[2] ?? match?.[3];
}

/** The text a `url("data:<type>…")` value carries, base64 or percent-encoded, when its type matches. */
function dataUrlText(value: string, type: RegExp): string | null {
  const url = urlOf(value);
  if (url === undefined) return null;
  const data = /^data:([^;,]*)((?:;[^,;]*)*),(.*)$/is.exec(url);
  if (data === null || !type.test(data[1]!)) return null;
  try {
    if (data[2]!.toLowerCase().split(";").includes("base64")) {
      const binary = atob(data[3]!.replace(/\s+/g, ""));
      const bytes = Uint8Array.from(binary, character => character.charCodeAt(0));
      return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    }
    return decodeURIComponent(data[3]!);
  } catch {
    return null;
  }
}

/**
 * The SVG markup of a `--theme-scene` value, when it is an SVG data URL
 * (`url("data:image/svg+xml;base64,…")` or the percent-encoded form). Any
 * other picture, or one that does not decode, stays a CSS background.
 */
export function sceneSvgSource(value: string): string | null {
  return dataUrlText(value, /^image\/svg\+xml$/i);
}

/**
 * The GLSL of a `--theme-shader` value: a fragment shader as a data URL,
 * `url("data:x-shader/x-fragment;base64,…")` (`text/plain` reads too).
 */
export function shaderSource(value: string): string | null {
  const source = dataUrlText(value, /^(?:x-shader\/x-fragment|text\/plain)$/i);
  return source === null || source.trim() === "" ? null : source;
}

/** The picture a `--theme-shader-image` value embeds, as the `data:image/…` URL to decode. */
export function shaderPicture(value: string): string | null {
  const url = urlOf(value);
  return url !== undefined && /^data:image\/(?:png|jpeg|webp|gif|avif)[;,]/i.test(url) ? url : null;
}

/** What the theme on `styles` asks of a shader, or nothing when it sets none that reads. */
export function shaderSettings(styles: Styles): ShaderSettings | null {
  const source = shaderSource(styles.getPropertyValue("--theme-shader"));
  if (source === null) return null;
  return {
    source,
    picture: shaderPicture(styles.getPropertyValue("--theme-shader-image")),
    scale: shaderNumber(styles.getPropertyValue("--theme-shader-scale"), SHADER_DEFAULT_SCALE, 0.1, 2),
    // No slower than ten a second: a step is at most BACKDROP_LONGEST_STEP_MS long.
    rate: shaderNumber(styles.getPropertyValue("--theme-shader-rate"), SHADER_DEFAULT_RATE, 10,
      BACKDROP_FRAMES_PER_SECOND),
    start: shaderNumber(styles.getPropertyValue("--theme-shader-start"), 0, 0, 86_400),
  };
}

const SVG_NAMESPACE = "http://www.w3.org/2000/svg";

/** What the scene cleaner reads and removes; a DOM `Element` is one. */
export interface SceneNode {
  readonly localName: string;
  readonly namespaceURI?: string | null;
  readonly children: ArrayLike<SceneNode>;
  getAttributeNames(): string[];
  getAttribute(name: string): string | null;
  removeAttribute(name: string): void;
  remove(): void;
}

/** Shapes, paint servers, filters, text and animation: drawing only. */
const SCENE_ELEMENTS: ReadonlySet<string> = new Set([
  "svg", "g", "defs", "symbol", "use", "switch", "title", "desc", "style",
  "path", "rect", "circle", "ellipse", "line", "polyline", "polygon",
  "text", "tspan", "textPath", "image",
  "linearGradient", "radialGradient", "stop", "pattern", "clipPath", "mask", "marker", "filter",
  "animate", "animateTransform", "animateMotion", "mpath", "set",
]);
const SCENE_ANIMATORS: ReadonlySet<string> = new Set(["animate", "animateTransform", "set"]);
const SCENE_PICTURES: ReadonlySet<string> = new Set(["image", "feImage"]);

function sceneElementAllowed(name: string): boolean {
  return SCENE_ELEMENTS.has(name) || /^fe[A-Z][A-Za-z]*$/.test(name);
}

function isLink(attribute: string): boolean {
  const name = attribute.toLowerCase();
  return name === "href" || name.endsWith(":href");
}

function foreignNamespace(node: SceneNode): boolean {
  return node.namespaceURI !== undefined && node.namespaceURI !== SVG_NAMESPACE;
}

/**
 * Leaves a scene only what draws. Scripts, foreign content, links and event
 * handlers go; a reference may point inside the scene (`#id`) or, for a
 * picture, at an embedded `data:image/`; an animation may not rewrite one.
 * The window's content policy refuses anything remote besides.
 */
export function cleanScene(node: SceneNode): void {
  if (foreignNamespace(node)) {
    node.remove();
    return;
  }
  for (const child of Array.from(node.children)) {
    const attributeName = (child.getAttribute("attributeName") ?? "").trim();
    const animatesLink = SCENE_ANIMATORS.has(child.localName) && /href$/i.test(attributeName);
    if (foreignNamespace(child) || !sceneElementAllowed(child.localName) || animatesLink) {
      child.remove();
      continue;
    }
    cleanScene(child);
  }
  for (const name of node.getAttributeNames()) {
    if (/^on/i.test(name)) {
      node.removeAttribute(name);
      continue;
    }
    if (!isLink(name)) continue;
    const target = (node.getAttribute(name) ?? "").trim();
    const embedded = SCENE_PICTURES.has(node.localName) && /^data:image\//i.test(target);
    if (!target.startsWith("#") && !embedded) node.removeAttribute(name);
  }
}

/** Settings › Appearance: backdrops move, move only on the power adapter, or stay still. */
export type ThemeMotionPreference = "on" | "plugged" | "off";

/** How often a moving backdrop is drawn on the power adapter. */
export const BACKDROP_FRAMES_PER_SECOND = 30;
/** How often on the battery: a frame costs about the same whatever it shows, so half as many cost half. */
export const BACKDROP_BATTERY_FRAMES_PER_SECOND = 15;
/** On the battery the backdrop rests once nothing was typed, clicked, moved or scrolled for this long. */
export const BACKDROP_BATTERY_IDLE_MS = 60_000;

export interface BackdropMotionState {
  windowActive: boolean;
  reducedMotion: boolean;
  preference: ThemeMotionPreference;
  /** Running on the battery, not the power adapter. Unknown counts as the adapter. */
  onBattery?: boolean;
  /** Milliseconds since the last key, pointer or wheel input in the window. */
  idleMs?: number;
}

/** How many times a second the backdrop is drawn now; zero while it rests. */
export function backdropFrameRate(state: BackdropMotionState): number {
  if (!state.windowActive || state.reducedMotion || state.preference === "off") return 0;
  if (state.onBattery !== true) return BACKDROP_FRAMES_PER_SECOND;
  if (state.preference === "plugged") return 0;
  if ((state.idleMs ?? 0) >= BACKDROP_BATTERY_IDLE_MS) return 0;
  return BACKDROP_BATTERY_FRAMES_PER_SECOND;
}

/** Whether a backdrop's animations run now. */
export function backdropMoves(state: BackdropMotionState): boolean {
  return backdropFrameRate(state) > 0;
}

/**
 * Whether a blur length blurs at all. A zero is left out of the page rather
 * than written as `blur(0px)`, which Chromium still composites as a filter.
 */
export function drawsBlur(length: string): boolean {
  const value = length.trim();
  return value !== "" && !/^[+-]?(?:0+\.?0*|\.0+)(?:[a-z]+|%)?$/iu.test(value);
}

/** The longest step one frame takes, so a stalled timer never lurches the scene. */
export const BACKDROP_LONGEST_STEP_MS = 100;

/** The part of an animation the stepper reads and moves; an `Animation` is one. */
export interface SteppedAnimation {
  readonly playState: AnimationPlayState;
  currentTime: CSSNumberish | null;
}

/**
 * Moves every held animation's clock forward by `elapsed` milliseconds and
 * says how far that was. Only paused ones move: theme-look.css holds the
 * backdrop's animations, and anything running on its own keeps its own time.
 */
export function stepBackdrop(animations: Iterable<SteppedAnimation>, elapsed: number): number {
  const step = Math.min(Math.max(elapsed, 0), BACKDROP_LONGEST_STEP_MS);
  if (step === 0) return 0;
  for (const animation of animations) {
    if (animation.playState === "paused") animation.currentTime = Number(animation.currentTime ?? 0) + step;
  }
  return step;
}

/** How often the sidebar's frosted copy catches up with a moving backdrop. */
export const FROST_REFRESH_MS = 500;
/** How often on the battery. */
export const FROST_BATTERY_REFRESH_MS = 1000;

/** Whether the frosted copy, last brought up to date at `refreshedAt`, is due again at `now`. */
export function frostRefreshDue(now: number, refreshedAt: number, onBattery: boolean): boolean {
  return now - refreshedAt >= (onBattery ? FROST_BATTERY_REFRESH_MS : FROST_REFRESH_MS);
}

/**
 * Each animation of the frosted copy with the backdrop's own animation it
 * follows. Both lists come keyed by where the animated element sits and which
 * keyframes it runs; the copy is a clone, so the keys agree. A copy animation
 * without a partner is left out and stays where it is.
 */
export function pairFrostAnimations<T>(
  copies: ReadonlyArray<readonly [key: string, animation: T]>,
  lives: ReadonlyArray<readonly [key: string, animation: T]>,
): Array<readonly [copy: T, live: T]> {
  const waiting = new Map<string, T[]>();
  for (const [key, animation] of lives) {
    const same = waiting.get(key);
    if (same === undefined) waiting.set(key, [animation]); else same.push(animation);
  }
  const pairs: Array<readonly [T, T]> = [];
  for (const [key, animation] of copies) {
    const live = waiting.get(key)?.shift();
    if (live !== undefined) pairs.push([animation, live]);
  }
  return pairs;
}

/* The scene's animations are held like the layers'; the stepper moves both. */
const SCENE_SHEET = `
:host { display: block; }
svg { position: absolute; inset: 0; display: block; width: 100%; height: 100%; }
*, *::before, *::after { animation-play-state: paused !important; }
`;

let backdrop: HTMLElement | undefined;
let sceneSource: string | null = null;
let sceneSvg: SVGSVGElement | undefined;
let motionPreference: ThemeMotionPreference = "on";
let stopWatching: (() => void) | undefined;

/* The stepper. A timer wakes a little before a frame is due and asks for the
   display's next frame; the step is taken there, so each drawn frame holds
   exactly one step and a window that is not being drawn takes none. */
/** How long before a frame is due the timer wakes: under one refresh at 120 Hz. */
const BACKDROP_FRAME_LEAD_MS = 6;
/** How often the list of animations is read again; they change only with the theme. */
const BACKDROP_REFRESH_MS = 2000;
/** Frames a second now; zero while resting. */
let frameRate = 0;
let stepTimer: number | undefined;
let stepRequest: number | undefined;
let steppedAt = 0;
let animations: SteppedAnimation[] = [];
let animationsReadAt = Number.NEGATIVE_INFINITY;
/** The scene carries SMIL animation, which has its own clock. */
let sceneTimed = false;
/** The theme's shader, drawing on its canvas under the scene. */
let shader: MountedShader | undefined;
/** The look has nothing to animate, found by looking: no timer runs until it changes. */
let still = false;
/** The sidebar's frost: a still copy of the art, placed under the sidebar by theme-look.css. */
let frost: HTMLElement | undefined;
/** The copy's own scene, when the scene is SVG. */
let frostSvg: SVGSVGElement | undefined;
/** What the copy was made from: it is made again only when one of these changes. */
let frostScene: string | null | undefined;
let frostShader: MountedShader | undefined;
/** Each of the copy's animations with the one it follows; read again when the backdrop's are. */
let frostPairs: Array<readonly [SteppedAnimation, SteppedAnimation]> | undefined;
let frostRefreshedAt = Number.NEGATIVE_INFINITY;

/** `navigator.getBattery()`'s answer, as far as this reads it. On a Mac
 * `charging` is true whenever the power adapter is attached. */
interface PowerSource extends EventTarget { readonly charging: boolean }
let powerSource: PowerSource | undefined;
let lastInputAt = 0;
let idleTimer: number | undefined;
/** Resting only because nothing was touched for a while on the battery: any input resumes. */
let idleResting = false;

function child(parent: HTMLElement, className: string): HTMLElement {
  const element = document.createElement("div");
  element.className = className;
  parent.append(element);
  return element;
}

/** The backdrop's elements: art (the scene, then the numbered layers), then the veil.
 * A theme's shader adds its canvas at the front of the art, under the scene. */
export function createThemeBackdrop(): HTMLElement {
  const root = document.createElement("div");
  root.className = "theme-backdrop";
  root.setAttribute("aria-hidden", "true");
  const art = child(root, "theme-backdrop-art");
  child(art, "theme-backdrop-scene");
  for (let layer = 1; layer <= THEME_BACKDROP_LAYERS; layer += 1) {
    child(art, "theme-backdrop-layer").dataset.layer = String(layer);
  }
  child(root, "theme-backdrop-veil");
  return root;
}

/** The sidebar's frost: one pane, blurred by theme-look.css, that holds the still copy of the art. */
export function createThemeFrost(): HTMLElement {
  const root = document.createElement("div");
  root.className = "theme-backdrop-frost";
  root.setAttribute("aria-hidden", "true");
  child(root, "theme-backdrop-frost-pane");
  return root;
}

/** The cleaned `<svg>` of a scene's markup, or nothing when it is not one. */
function parseScene(source: string): SVGSVGElement | undefined {
  const parsed = new DOMParser().parseFromString(source, "image/svg+xml");
  const svg = parsed.documentElement;
  if (svg.localName !== "svg" || foreignNamespace(svg)
      || parsed.querySelector("parsererror") !== null) return undefined;
  cleanScene(svg);
  // Fill the window whatever its shape, the way a cover background does.
  if (!svg.hasAttribute("preserveAspectRatio")) svg.setAttribute("preserveAspectRatio", "xMidYMid slice");
  svg.removeAttribute("width");
  svg.removeAttribute("height");
  return document.importNode(svg, true) as unknown as SVGSVGElement;
}

/** Draws an SVG scene as real SVG inside the scene layer, behind a shadow
 * boundary so its styles and the app's never meet. */
function paintScene(host: HTMLElement, source: string | null): void {
  if (source === sceneSource && (source === null || sceneSvg?.isConnected === true)) return;
  sceneSource = source;
  sceneSvg = source === null ? undefined : parseScene(source);
  sceneTimed = false;
  const shadow = host.shadowRoot ?? host.attachShadow({ mode: "open" });
  shadow.replaceChildren();
  if (sceneSvg === undefined) {
    // Not SVG: theme-look.css paints --theme-scene as a cover background.
    host.style.removeProperty("background");
    return;
  }
  const sheet = document.createElement("style");
  sheet.textContent = SCENE_SHEET;
  shadow.append(sheet, sceneSvg);
  host.style.setProperty("background", "none");
  // SMIL has its own clock, held here and moved by the stepper.
  sceneSvg.pauseAnimations?.();
  sceneTimed = sceneSvg.querySelector("animate, animateTransform, animateMotion, set") !== null;
}

/** Draws the theme's shader on a canvas under the scene, or takes the canvas away. */
function paintShader(art: HTMLElement, settings: ShaderSettings | null): void {
  const canvas = art.querySelector<HTMLCanvasElement>(":scope > .theme-backdrop-shader");
  if (shader !== undefined && settings !== null && canvas !== null && sameShader(shader.settings, settings)) return;
  shader?.dispose();
  shader = undefined;
  canvas?.remove();
  if (settings === null) return;
  const fresh = document.createElement("canvas");
  fresh.className = "theme-backdrop-shader";
  art.prepend(fresh);
  // A shader draws by itself when it starts, when its picture arrives and when
  // the window changes size: the frosted copy takes that picture too.
  shader = mountThemeShader(fresh, settings, () => { refreshFrost(performance.now()); });
}

/** Where an animated element sits under `root` and which keyframes it runs. */
function frostKey(animation: Animation, root: Node): string {
  const effect = animation.effect as KeyframeEffect | null;
  const path: number[] = [];
  let node: Node | null = effect?.target ?? null;
  while (node !== null && node !== root) {
    const parent: Node | null = node.parentNode;
    if (parent === null) break;
    path.push(Array.prototype.indexOf.call(parent.childNodes, node) as number);
    node = parent;
  }
  const name = (animation as Partial<CSSAnimation>).animationName ?? animation.id;
  return `${path.reverse().join(".")}|${effect?.pseudoElement ?? ""}|${name}`;
}

function keyedAnimations(root: Element | undefined | null): Array<readonly [string, Animation]> {
  if (root === undefined || root === null) return [];
  return root.getAnimations({ subtree: true }).map(animation => [frostKey(animation, root), animation] as const);
}

/**
 * Makes the frosted copy of the art: the same layers under the same theme
 * rules, the scene drawn a second time, and a plain canvas for the shader's
 * picture. Made again only when the scene or the shader changed; the theme's
 * CSS reaches the copy by the same class names as it reaches the art.
 */
function paintFrost(): void {
  if (backdrop === undefined || frost === undefined) return;
  if (frostScene === sceneSource && frostShader === shader && frost.querySelector(".theme-backdrop-art") !== null) return;
  frostScene = sceneSource;
  frostShader = shader;
  frostPairs = undefined;
  frostSvg = undefined;
  const art = backdrop.querySelector<HTMLElement>(".theme-backdrop-art")!;
  const copy = art.cloneNode(true) as HTMLElement;
  if (sceneSvg !== undefined) {
    // A clone leaves the scene's shadow root behind: the copy draws its own.
    frostSvg = sceneSvg.cloneNode(true) as SVGSVGElement;
    const sheet = document.createElement("style");
    sheet.textContent = SCENE_SHEET;
    copy.querySelector<HTMLElement>(".theme-backdrop-scene")!.attachShadow({ mode: "open" }).append(sheet, frostSvg);
    frostSvg.pauseAnimations?.();
  }
  const veil = backdrop.querySelector<HTMLElement>(".theme-backdrop-veil")!.cloneNode(false) as HTMLElement;
  frost.querySelector<HTMLElement>(".theme-backdrop-frost-pane")!.replaceChildren(copy, veil);
}

/** The copy's animations with the ones they follow. Like the art's, none runs on its own clock. */
function readFrostPairs(): Array<readonly [SteppedAnimation, SteppedAnimation]> {
  if (backdrop === undefined || frost === undefined) return [];
  const art = backdrop.querySelector(".theme-backdrop-art");
  const copy = frost.querySelector(".theme-backdrop-art");
  const copies = [...keyedAnimations(copy), ...keyedAnimations(frostSvg)];
  for (const [, animation] of copies) if (animation.playState === "running") animation.pause();
  return pairFrostAnimations<Animation>(copies, [...keyedAnimations(art), ...keyedAnimations(sceneSvg)]);
}

/**
 * Brings the frosted copy up to the picture the backdrop shows now: every
 * animation of the copy jumps to its partner's time, and the shader's picture
 * is drawn across. Called a couple of times a second while the backdrop
 * moves, and once whenever it comes to rest or is painted afresh.
 */
function refreshFrost(now: number): void {
  frostRefreshedAt = now;
  if (backdrop === undefined || frost === undefined) return;
  frostPairs ??= readFrostPairs();
  for (const [copy, live] of frostPairs) copy.currentTime = live.currentTime;
  if (sceneTimed && frostSvg?.setCurrentTime !== undefined && sceneSvg?.getCurrentTime !== undefined) {
    frostSvg.setCurrentTime(sceneSvg.getCurrentTime());
  }
  const canvas = frost.querySelector<HTMLCanvasElement>(".theme-backdrop-shader");
  if (canvas === null || shader === undefined) return;
  // Hidden while the shader cannot draw, like the canvas it copies.
  canvas.hidden = shader.failed;
  shader.copyTo(canvas);
}

/**
 * Every animation in the backdrop and its scene, each held by its own clock.
 * theme-look.css and SCENE_SHEET hold them in CSS, but a theme file or a
 * scene's own <style> can outrank that with a more specific
 * `animation-play-state: running !important`; an animation paused here is no
 * longer driven by CSS at all, so nothing in a file can set it running again.
 */
function heldAnimations(): Animation[] {
  if (backdrop === undefined) return [];
  const found: Animation[] = backdrop.getAnimations({ subtree: true });
  if (sceneSvg !== undefined) found.push(...sceneSvg.getAnimations({ subtree: true }));
  for (const animation of found) if (animation.playState === "running") animation.pause();
  return found;
}

/** Reads which animations there are to step, and whether there is anything to move at all. */
function readAnimations(now: number): boolean {
  animationsReadAt = now;
  frostPairs = undefined;
  if (backdrop === undefined) return false;
  animations = heldAnimations().filter(animation => animation.playState === "paused");
  // A shader that cannot draw has nothing to move: its plain backdrop shows.
  if (shader?.failed === true) return animations.length > 0 || sceneTimed;
  return animations.length > 0 || sceneTimed || shader !== undefined;
}

function scheduleStep(delay: number): void {
  stepTimer = window.setTimeout(() => {
    stepTimer = undefined;
    stepRequest = window.requestAnimationFrame(stepFrame);
  }, Math.max(0, delay));
}

/** One drawn frame: every held animation moves on by the time since the last. */
function stepFrame(now: number): void {
  stepRequest = undefined;
  if (backdrop === undefined || frameRate === 0) return;
  if (now - animationsReadAt >= BACKDROP_REFRESH_MS && !readAnimations(now)) {
    still = true;
    syncMotion();
    return;
  }
  const step = stepBackdrop(animations, now - steppedAt);
  steppedAt = now;
  if (step > 0 && sceneTimed && sceneSvg?.setCurrentTime !== undefined) {
    sceneSvg.setCurrentTime(sceneSvg.getCurrentTime() + step / 1000);
  }
  if (step > 0) shader?.advance(step, now);
  // The sidebar's frosted copy follows a few times a second, never every frame.
  if (frost !== undefined && frostRefreshDue(now, frostRefreshedAt, powerSource !== undefined && !powerSource.charging)) {
    refreshFrost(now);
  }
  // A shader with nothing else moving sets the pace itself, when it asks for less.
  const rate = shader !== undefined && animations.length === 0 && !sceneTimed
    ? Math.min(frameRate, shader.settings.rate) : frameRate;
  scheduleStep(1000 / rate - BACKDROP_FRAME_LEAD_MS - (performance.now() - now));
}

function setStepping(rate: number): void {
  const resting = frameRate === 0;
  frameRate = rate;
  if (rate === 0) {
    if (stepTimer !== undefined) window.clearTimeout(stepTimer);
    if (stepRequest !== undefined) window.cancelAnimationFrame(stepRequest);
    stepTimer = undefined;
    stepRequest = undefined;
    // Come to rest, the frosted copy shows the picture the backdrop stopped on.
    if (!resting) refreshFrost(performance.now());
    return;
  }
  if (!resting) return;
  steppedAt = performance.now();
  animationsReadAt = Number.NEGATIVE_INFINITY;
  scheduleStep(0);
}

/** Runs or rests the backdrop's motion from what the document shows now. */
function syncMotion(): void {
  const root = document.documentElement;
  if (idleTimer !== undefined) window.clearTimeout(idleTimer);
  idleTimer = undefined;
  idleResting = false;
  if (backdrop === undefined) {
    delete root.dataset.themeMotion;
    setStepping(0);
    return;
  }
  // Resting or not, nothing in the backdrop runs on its own clock.
  heldAnimations();
  const idleMs = performance.now() - lastInputAt;
  const state: BackdropMotionState = {
    windowActive: root.dataset.windowActive !== "false",
    reducedMotion: window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    preference: motionPreference,
    onBattery: powerSource !== undefined && !powerSource.charging,
    idleMs,
  };
  const rate = still ? 0 : backdropFrameRate(state);
  if (rate > 0 && state.onBattery === true) {
    idleTimer = window.setTimeout(syncMotion, BACKDROP_BATTERY_IDLE_MS - idleMs + 20);
  }
  idleResting = !still && rate === 0 && backdropFrameRate({ ...state, idleMs: 0 }) > 0;
  root.dataset.themeMotion = rate > 0 ? "on" : "rest";
  setStepping(rate);
}

/** A key, the pointer or the wheel: the minute the battery rule counts starts over. */
function noteInput(): void {
  lastInputAt = performance.now();
  if (idleResting) syncMotion();
}

/** The theme, the window's size or the motion setting changed: look again for something to move. */
function lookAgain(): void {
  still = false;
  animationsReadAt = Number.NEGATIVE_INFINITY;
  syncMotion();
  // The theme's rules may have changed what moves: the copy is paired and brought up to date again.
  frostPairs = undefined;
  refreshFrost(performance.now());
  roundFrost();
}

/** The frost's corners follow the sidebar's, which differ between interface styles. */
function roundFrost(): void {
  if (frost === undefined || typeof document.querySelector !== "function") return;
  const sidebar = document.querySelector(".app-grid > .sidebar");
  const radius = sidebar === null ? undefined : getComputedStyle(sidebar).borderTopLeftRadius;
  if (typeof radius === "string" && radius !== "") frost.style.setProperty("--theme-frost-radius", radius);
}

const INPUT_EVENTS = ["keydown", "pointerdown", "pointermove", "wheel"] as const;

function watchMotion(): () => void {
  let stopped = false;
  // Coming back to Interface counts as touching it.
  const observer = new MutationObserver(() => {
    lastInputAt = performance.now();
    syncMotion();
  });
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-window-active"] });
  const media = window.matchMedia("(prefers-reduced-motion: reduce)");
  media.addEventListener("change", lookAgain);
  window.addEventListener("resize", lookAgain);
  for (const type of INPUT_EVENTS) window.addEventListener(type, noteInput, { capture: true, passive: true });
  lastInputAt = performance.now();
  const battery = (navigator as Navigator & { getBattery?: () => Promise<PowerSource> }).getBattery;
  if (powerSource === undefined && typeof battery === "function") {
    void battery.call(navigator).then(source => {
      if (stopped) return;
      powerSource = source;
      source.addEventListener("chargingchange", syncMotion);
      syncMotion();
    }, () => { /* No battery to ask: drawn as on the power adapter. */ });
  }
  return () => {
    stopped = true;
    observer.disconnect();
    media.removeEventListener("change", lookAgain);
    window.removeEventListener("resize", lookAgain);
    for (const type of INPUT_EVENTS) window.removeEventListener(type, noteInput, { capture: true });
    powerSource?.removeEventListener("chargingchange", syncMotion);
    powerSource = undefined;
  };
}

/** Whether the painted theme draws a backdrop: Settings shows its motion switch then. */
export function themeLookPainted(): boolean {
  return typeof document !== "undefined" && document.documentElement.dataset.themeLook === "backdrop";
}

/** Settings' switch: whether backdrops move at all. */
export function applyThemeMotion(preference: ThemeMotionPreference): void {
  motionPreference = preference;
  lookAgain();
}

/**
 * Puts the backdrop under the document, or takes it away, and marks the root
 * so the glass rules apply. Called after the theme file's CSS is adopted, so
 * the root's computed style already reads the file's values.
 */
export function applyThemeLook(themeFilePainted: boolean): boolean {
  const root = document.documentElement;
  // A built-in palette never draws one, so its styles are not read at all.
  const styles = themeFilePainted && typeof getComputedStyle === "function"
    ? getComputedStyle(root) : undefined;
  if (styles === undefined || !themeDrawsBackdrop(styles)) {
    delete root.dataset.themeLook;
    delete root.dataset.themeFrost;
    backdrop?.remove();
    backdrop = undefined;
    frost?.remove();
    frost = undefined;
    frostSvg = undefined;
    frostScene = undefined;
    frostShader = undefined;
    frostPairs = undefined;
    sceneSource = null;
    sceneSvg = undefined;
    sceneTimed = false;
    shader?.dispose();
    shader = undefined;
    animations = [];
    stopWatching?.();
    stopWatching = undefined;
    syncMotion();
    return false;
  }
  root.dataset.themeLook = "backdrop";
  if (backdrop === undefined || !backdrop.isConnected) {
    backdrop = createThemeBackdrop();
    document.body.prepend(backdrop);
    sceneSource = null;
  }
  if (frost === undefined || !frost.isConnected) {
    frost = createThemeFrost();
    backdrop.after(frost);
    frostScene = undefined;
  }
  // A blur is written into the page only when the file asks for one.
  backdrop.toggleAttribute("data-blur", drawsBlur(styles.getPropertyValue("--theme-backdrop-blur")));
  root.toggleAttribute("data-theme-frost", drawsBlur(styles.getPropertyValue("--theme-glass-blur")));
  paintScene(backdrop.querySelector<HTMLElement>(".theme-backdrop-scene")!,
    sceneSvgSource(styles.getPropertyValue("--theme-scene")));
  paintShader(backdrop.querySelector<HTMLElement>(".theme-backdrop-art")!, shaderSettings(styles));
  paintFrost();
  stopWatching ??= watchMotion();
  lookAgain();
  return true;
}
