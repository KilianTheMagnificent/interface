export interface RgbColor { r: number; g: number; b: number }

export function parseHexColor(value: string): RgbColor {
  const hex = value.trim().replace(/^#/u, "");
  if (!/^[0-9a-f]{6}$/iu.test(hex)) throw new Error(`Expected a six-digit hex color: ${value}`);
  return {
    r: Number.parseInt(hex.slice(0, 2), 16),
    g: Number.parseInt(hex.slice(2, 4), 16),
    b: Number.parseInt(hex.slice(4, 6), 16),
  };
}

const linearChannel = (channel: number): number => {
  const normalized = channel / 255;
  return normalized <= 0.04045
    ? normalized / 12.92
    : ((normalized + 0.055) / 1.055) ** 2.4;
};

export function relativeLuminance(value: string): number {
  const { r, g, b } = parseHexColor(value);
  return 0.2126 * linearChannel(r)
    + 0.7152 * linearChannel(g)
    + 0.0722 * linearChannel(b);
}

/** WCAG relative-luminance contrast ratio. */
export function contrastRatio(foreground: string, background: string): number {
  const first = relativeLuminance(foreground);
  const second = relativeLuminance(background);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}

/** Composite an eight-digit #rrggbbaa token over an opaque six-digit background. */
export function compositeHex(foreground: string, background: string): string {
  const value = foreground.trim().replace(/^#/u, "");
  if (!/^[0-9a-f]{8}$/iu.test(value)) {
    throw new Error(`Expected an eight-digit hex color: ${foreground}`);
  }
  const front = parseHexColor(`#${value.slice(0, 6)}`);
  const back = parseHexColor(background);
  const alpha = Number.parseInt(value.slice(6), 16) / 255;
  const channel = (frontValue: number, backValue: number): string =>
    Math.round(frontValue * alpha + backValue * (1 - alpha))
      .toString(16).padStart(2, "0");
  return `#${channel(front.r, back.r)}${channel(front.g, back.g)}${channel(front.b, back.b)}`;
}

interface Rgba { r: number; g: number; b: number; a: number }

const toLinear = (v: number): number => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const fromLinear = (v: number): number => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);

function toOklab({ r, g, b }: Rgba): [number, number, number] {
  const [lr, lg, lb] = [toLinear(r), toLinear(g), toLinear(b)];
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return [
    0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
  ];
}

function fromOklab([L, A, B]: [number, number, number]): Omit<Rgba, "a"> {
  const l = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3;
  const m = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3;
  const s = (L - 0.0894841775 * A - 1.2914855480 * B) ** 3;
  const clamp = (v: number): number => Math.min(1, Math.max(0, fromLinear(v)));
  return {
    r: clamp(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    g: clamp(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    b: clamp(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
  };
}

function splitTopLevel(text: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  for (let at = 0; at < text.length; at += 1) {
    if (text[at] === "(") depth += 1;
    else if (text[at] === ")") depth -= 1;
    else if (text[at] === "," && depth === 0) { parts.push(text.slice(start, at).trim()); start = at + 1; }
  }
  parts.push(text.slice(start).trim());
  return parts;
}

function parseRgba(value: string): Rgba {
  const text = value.trim();
  if (text === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  // The one relative colour theme.css uses: a colour's lightness as a neutral grey.
  const grey = /^oklch\(from (.+) l 0 0\)$/u.exec(text);
  if (grey !== null) {
    const source = parseRgba(grey[1]!);
    return { ...fromOklab([toOklab(source)[0], 0, 0]), a: source.a };
  }
  if (text.startsWith("color-mix(") && text.endsWith(")")) {
    const [space, first, second] = splitTopLevel(text.slice("color-mix(".length, -1));
    if (second === undefined || !/^in (srgb|oklab)$/u.test(space ?? "")) {
      throw new Error(`Unsupported color-mix: ${value}`);
    }
    const stop = (part: string): { colour: Rgba; weight: number | null } => {
      const match = /^(.*?)\s+(\d+(?:\.\d+)?)%$/u.exec(part);
      return match === null ? { colour: parseRgba(part), weight: null }
        : { colour: parseRgba(match[1]!), weight: Number(match[2]) / 100 };
    };
    const one = stop(first!);
    const two = stop(second);
    let p1 = one.weight ?? (two.weight === null ? 0.5 : 1 - two.weight);
    let p2 = two.weight ?? 1 - p1;
    const total = p1 + p2;
    const alphaScale = Math.min(1, total);
    p1 /= total;
    p2 /= total;
    const alpha = one.colour.a * p1 + two.colour.a * p2;
    if (alpha === 0) return { r: 0, g: 0, b: 0, a: 0 };
    if (space === "in srgb") {
      const channel = (key: "r" | "g" | "b"): number =>
        (one.colour[key] * one.colour.a * p1 + two.colour[key] * two.colour.a * p2) / alpha;
      return { r: channel("r"), g: channel("g"), b: channel("b"), a: alpha * alphaScale };
    }
    const [a1, a2] = [toOklab(one.colour), toOklab(two.colour)];
    const mixed = [0, 1, 2].map(index =>
      (a1[index]! * one.colour.a * p1 + a2[index]! * two.colour.a * p2) / alpha) as [number, number, number];
    return { ...fromOklab(mixed), a: alpha * alphaScale };
  }
  const hex = text.replace(/^#/u, "");
  const full = hex.length === 3 || hex.length === 4 ? [...hex].map(c => c + c).join("") : hex;
  if (!/^[0-9a-f]{6}(?:[0-9a-f]{2})?$/iu.test(full)) throw new Error(`Unsupported colour: ${value}`);
  const byte = (offset: number): number => Number.parseInt(full.slice(offset, offset + 2), 16) / 255;
  return { r: byte(0), g: byte(2), b: byte(4), a: full.length === 8 ? byte(6) : 1 };
}

/**
 * The hex a browser paints for the colour values theme.css's theme-file
 * palettes use: hex, `transparent` and `color-mix(in srgb|oklab, …)`, nested.
 * Premultiplied, with percentages normalised, as CSS Color 5 specifies.
 * Returns #rrggbb, or #rrggbbaa when the result is not opaque.
 */
export function evaluateCssColor(value: string): string {
  const { r, g, b, a } = parseRgba(value);
  const byte = (v: number): string => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, "0");
  return `#${byte(r)}${byte(g)}${byte(b)}${a >= 0.999 ? "" : byte(a)}`;
}
