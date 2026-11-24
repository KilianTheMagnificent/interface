/**
 * Point-and-fix in the Browser pane: what a picked element or a marked region
 * captured, made safe to show the owner and to put in a thread's draft.
 *
 * The picker runs in the page's own main world (`browser-point-page.ts`), so
 * everything it hands back is untrusted page data: a page can return any
 * shape, any size and any characters. Nothing here trusts a type, a length or
 * a character; text is only ever drawn as text, never as HTML, and the card
 * shows it to the owner before any of it reaches a draft.
 */

import { PROMPT_IMAGE_LIMITS } from "../shared/prompt-images";

export const BROWSER_POINT_LIMITS = {
  /** A selector longer than this is dropped, never cut: a cut selector names the wrong thing. */
  selector: 300,
  text: 200,
  name: 200,
  title: 200,
  url: 2_048,
  comment: 4_000,
  tag: 64,
  role: 40,
  /** CSS pixels; anything past this is not a real layout box. */
  coordinate: 1_000_000,
  viewport: 100_000,
  /** The space kept around a picked element in its picture, in CSS pixels. */
  margin: 8,
  /** What `savePastedImages` admits at all. */
  imageBytes: PROMPT_IMAGE_LIMITS.maxSourceBytes,
} as const;

export interface BrowserBox { readonly x: number; readonly y: number; readonly width: number; readonly height: number }
export interface BrowserViewportSize { readonly width: number; readonly height: number }

export interface BrowserElementPick {
  readonly kind: "element";
  /** Null when the page offered none that is short and unique. */
  readonly selector: string | null;
  readonly tag: string;
  readonly text: string;
  /** Accessible name and role, when the element has them. */
  readonly name: string | null;
  readonly role: string | null;
  /** An embedded frame: its contents are out of the picker's reach. */
  readonly frame: boolean;
  readonly box: BrowserBox;
  readonly viewport: BrowserViewportSize;
}

export interface BrowserRegionPick {
  readonly kind: "region";
  readonly box: BrowserBox;
  readonly viewport: BrowserViewportSize;
}

export type BrowserPick = BrowserElementPick | BrowserRegionPick;

/** A pick and the page it was made on. */
export interface BrowserCapture {
  readonly pick: BrowserPick;
  readonly url: string;
  readonly title: string;
}

// C0 and C1 controls, and the characters that reorder or hide text: a page
// must not be able to make the card say one thing and the draft another.
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/gu;
const INVISIBLE = /[­؜᠎​-‏‪-‮⁠-⁤⁦-⁯﻿￹-￻]/gu;
const LONE_SURROGATE = /[\ud800-\udfff]/gu;

/**
 * Page text as one plain line of at most `max` characters: controls become
 * spaces, invisible and reordering characters go, runs of space collapse,
 * and a longer text ends in an ellipsis. Anything but a string is empty.
 */
export function cleanPageText(value: unknown, max: number): string {
  if (typeof value !== "string" || max <= 0) return "";
  // Bound the work before any regex runs over a page-sized string.
  const head = value.slice(0, max * 4 + 16);
  const flat = head.replace(CONTROL, " ").replace(INVISIBLE, "").replace(LONE_SURROGATE, "")
    .replace(/\s+/gu, " ").trim();
  const points = Array.from(flat);
  if (points.length <= max && head.length === value.length) return flat;
  return `${points.slice(0, max - 1).join("").trimEnd()}…`;
}

/** A selector as the picker built it, or null when it is missing, empty or too long. */
export function cleanSelector(value: unknown): string | null {
  if (typeof value !== "string" || value.length > BROWSER_POINT_LIMITS.selector * 2) return null;
  const flat = value.replace(CONTROL, " ").replace(INVISIBLE, "").replace(LONE_SURROGATE, "")
    .replace(/\s+/gu, " ").trim();
  return flat === "" || flat.length > BROWSER_POINT_LIMITS.selector ? null : flat;
}

function cleanTag(value: unknown): string {
  if (typeof value !== "string") return "element";
  const tag = value.toLowerCase();
  return tag.length <= BROWSER_POINT_LIMITS.tag && /^[a-z][a-z0-9-]*$/u.test(tag) ? tag : "element";
}

function cleanRole(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const role = value.trim().toLowerCase();
  return role.length > 0 && role.length <= BROWSER_POINT_LIMITS.role && /^[a-z][a-z-]*$/u.test(role) ? role : null;
}

function finite(value: unknown, min: number, max: number): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  return Math.round(Math.min(max, Math.max(min, value)));
}

function cleanBox(value: unknown): BrowserBox | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  const limit = BROWSER_POINT_LIMITS.coordinate;
  const x = finite(raw.x, -limit, limit);
  const y = finite(raw.y, -limit, limit);
  const width = finite(raw.width, 0, limit);
  const height = finite(raw.height, 0, limit);
  return x === null || y === null || width === null || height === null ? null : { x, y, width, height };
}

function cleanViewport(value: unknown): BrowserViewportSize | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  const width = finite(raw.width, 0, BROWSER_POINT_LIMITS.viewport);
  const height = finite(raw.height, 0, BROWSER_POINT_LIMITS.viewport);
  return width === null || height === null || width < 1 || height < 1 ? null : { width, height };
}

/**
 * What the in-page picker returned, checked field by field, or null for a
 * cancel or anything that is not a pick. Strings are bounded and cleaned,
 * numbers must be finite and are clamped, and unknown fields are dropped.
 */
export function browserPickFromPage(raw: unknown): BrowserPick | null {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return null;
  const value = raw as Record<string, unknown>;
  const box = cleanBox(value.box);
  const viewport = cleanViewport(value.viewport);
  if (box === null || viewport === null) return null;
  if (value.kind === "region") {
    return box.width < 1 || box.height < 1 ? null : { kind: "region", box, viewport };
  }
  if (value.kind !== "element") return null;
  const name = cleanPageText(value.name, BROWSER_POINT_LIMITS.name);
  return {
    kind: "element",
    selector: cleanSelector(value.selector),
    tag: cleanTag(value.tag),
    text: cleanPageText(value.text, BROWSER_POINT_LIMITS.text),
    name: name === "" ? null : name,
    role: cleanRole(value.role),
    frame: value.frame === true,
    box,
    viewport,
  };
}

/** The page's address and title for a capture, bounded and cleaned. */
export function browserCapture(pick: BrowserPick, url: unknown, title: unknown): BrowserCapture {
  return {
    pick,
    url: cleanPageText(url, BROWSER_POINT_LIMITS.url),
    title: cleanPageText(title, BROWSER_POINT_LIMITS.title),
  };
}

/**
 * The part of the page view a capture's picture covers, in the guest's
 * device-independent pixels (`<webview>.capturePage`): the box plus a margin
 * for an element, clipped to the visible page and scaled by the page zoom.
 * Null when nothing of it is on screen.
 */
export function browserCropRect(pick: BrowserPick, zoom = 1): BrowserBox | null {
  const scale = Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
  const margin = pick.kind === "element" ? BROWSER_POINT_LIMITS.margin : 0;
  const { box, viewport } = pick;
  const left = Math.max(0, Math.floor(box.x - margin));
  const top = Math.max(0, Math.floor(box.y - margin));
  const right = Math.min(viewport.width, Math.ceil(box.x + box.width + margin));
  const bottom = Math.min(viewport.height, Math.ceil(box.y + box.height + margin));
  if (right - left < 1 || bottom - top < 1) return null;
  return {
    x: Math.round(left * scale), y: Math.round(top * scale),
    width: Math.max(1, Math.round((right - left) * scale)), height: Math.max(1, Math.round((bottom - top) * scale)),
  };
}

const PNG_DATA_URL = "data:image/png;base64,";

/**
 * A captured picture's PNG bytes from `NativeImage.toDataURL()`, or null for
 * anything that is not a PNG data URL or would be refused for its size.
 * (`toPNG()` needs Node's Buffer, which a sandboxed renderer does not have.)
 */
export function pngBytesFromDataUrl(dataUrl: unknown): Uint8Array | null {
  if (typeof dataUrl !== "string" || !dataUrl.startsWith(PNG_DATA_URL)) return null;
  const encoded = dataUrl.slice(PNG_DATA_URL.length);
  if (encoded.length === 0 || encoded.length > Math.ceil(BROWSER_POINT_LIMITS.imageBytes / 3) * 4) return null;
  let binary: string;
  try { binary = atob(encoded); } catch { return null; }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  const signature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  return signature.every((byte, index) => bytes[index] === byte) ? bytes : null;
}

const PLAIN_TAGS: Readonly<Record<string, string>> = {
  a: "link", button: "button", img: "image", svg: "drawing", input: "input field", textarea: "text box",
  select: "menu", label: "label", p: "paragraph", li: "list item", ul: "list", ol: "list", nav: "navigation bar",
  header: "header", footer: "footer", form: "form", table: "table", video: "video", canvas: "canvas",
  h1: "heading", h2: "heading", h3: "heading", h4: "heading", h5: "heading", h6: "heading",
};

/** "A button", "A heading": what the owner sees in the card. */
function plainElement(pick: BrowserElementPick): string {
  const word = PLAIN_TAGS[pick.tag] ?? (pick.role === "button" || pick.role === "link" ? pick.role : null);
  return word === null ? `A <${pick.tag}> element` : `${/^[aeiou]/u.test(word) ? "An" : "A"} ${word}`;
}

/** What was captured, in plain words, for the card. */
export function browserCaptureSummary(capture: BrowserCapture): string {
  const { pick } = capture;
  const size = `${pick.box.width.toString()}×${pick.box.height.toString()}`;
  if (pick.kind === "region") return `A region of the page, ${size}`;
  if (pick.frame) return "An embedded frame: can't look inside this embedded frame. Mark a region to show what it draws.";
  const label = pick.text !== "" ? pick.text : pick.name ?? "";
  return label === "" ? `${plainElement(pick)}, ${size}` : `${plainElement(pick)} “${label}”, ${size}`;
}

/** Inline code that survives backticks in the selector. */
function inlineCode(text: string): string {
  const longest = Math.max(0, ...[...text.matchAll(/`+/gu)].map(run => run[0].length));
  const fence = "`".repeat(longest + 1);
  const pad = text.startsWith("`") || text.endsWith("`") ? " " : "";
  return `${fence}${pad}${text}${pad}${fence}`;
}

/** The owner's comment: lines kept, other controls dropped, bounded. */
export function cleanComment(comment: string): string {
  return comment.slice(0, BROWSER_POINT_LIMITS.comment).replace(/\r\n?/gu, "\n")
    .replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/gu, " ").replace(INVISIBLE, "").trim();
}

/**
 * The note a capture puts in a draft:
 *
 *     In the browser at <url> ("<title>"):
 *     Element <tag> "<text>" (selector `<selector>`, <w>×<h> at <x>,<y>)
 *     > <comment>
 */
export function browserCaptureNote(capture: BrowserCapture, comment: string): string {
  const { pick } = capture;
  const where = `In the browser at ${capture.url || "an unknown page"}${capture.title ? ` ("${capture.title}")` : ""}:`;
  const place = `${pick.box.width.toString()}×${pick.box.height.toString()} at ${pick.box.x.toString()},${pick.box.y.toString()}`;
  let what: string;
  if (pick.kind === "region") {
    what = `Region ${place} of a ${pick.viewport.width.toString()}×${pick.viewport.height.toString()} view (picture attached)`;
  } else {
    const selector = pick.selector === null ? "" : `selector ${inlineCode(pick.selector)}, `;
    const text = pick.text === "" ? "" : ` "${pick.text}"`;
    const named = pick.name !== null && pick.name !== pick.text ? ` named "${pick.name}"` : "";
    what = pick.frame
      ? `Embedded frame <${pick.tag}> (${selector}${place}); its contents could not be inspected`
      : `Element ${pick.tag}${text}${named} (${selector}${place})`;
  }
  const said = cleanComment(comment);
  const quote = said === "" ? [] : said.split("\n").map(line => `> ${line}`.trimEnd());
  return [where, what, ...quote].join("\n");
}

/**
 * A thread's draft with a browser note added: after whatever is already
 * typed, one blank line apart, as a diff quote is (`appendDiffQuote`).
 */
export function appendBrowserNote(draft: string, note: string): string {
  const current = draft.replace(/\s+$/u, "");
  const block = `${note}\n\n`;
  return current === "" ? block : `${current}\n\n${block}`;
}

/** Where a capture can go: Output preview's feedback targets. */
export interface BrowserNoteTarget { readonly key: string; readonly title: string }

/** What became of the picture when a note was added to a draft. */
export type BrowserNoteImage = "attached" | "none" | "unsupported" | "failed";

export interface BrowserNoteDraftPort {
  readonly getDraft: (key: string) => string;
  readonly setDraft: (key: string, text: string) => void;
  /** False for a target whose composer takes no attachments (a chat with no project). */
  readonly canAttach: (key: string) => boolean;
  /** Saves the PNG as a pasted image on the target's draft; the paths it attached. */
  readonly attachImage: (key: string, png: Uint8Array) => Promise<readonly string[]>;
}

/**
 * "Add to draft": the note goes after the draft's words, and the picture
 * joins the draft as an image attachment, as a pasted screenshot does.
 * Nothing is sent; the owner reviews the draft and sends it.
 */
export async function addBrowserNoteToDraft(port: BrowserNoteDraftPort, key: string, note: string,
    png: Uint8Array | null): Promise<BrowserNoteImage> {
  port.setDraft(key, appendBrowserNote(port.getDraft(key), note));
  if (png === null) return "none";
  if (!port.canAttach(key)) return "unsupported";
  try {
    return (await port.attachImage(key, png)).length > 0 ? "attached" : "failed";
  } catch {
    return "failed";
  }
}

/** The line the pane shows once a note is in a draft. */
export function browserNoteAddedMessage(title: string, image: BrowserNoteImage): string {
  const where = `“${cleanPageText(title, 80) || "the thread"}”`;
  switch (image) {
    case "attached": return `Added to the draft in ${where}, with its picture. Review it there and send.`;
    case "none": return `Added to the draft in ${where}. Review it there and send.`;
    case "unsupported": return `Added to the draft in ${where} without its picture: that chat takes no attachments.`;
    case "failed": return `Added to the draft in ${where}, but its picture could not be attached.`;
  }
}

export type BrowserViewportId = "fit" | "phone" | "tablet" | "desktop";

export interface BrowserViewportPreset {
  readonly id: BrowserViewportId;
  readonly label: string;
  /** CSS pixels; null fills the pane. */
  readonly width: number | null;
}

export const BROWSER_VIEWPORTS: readonly BrowserViewportPreset[] = [
  { id: "fit", label: "Fit", width: null },
  { id: "phone", label: "Phone 390", width: 390 },
  { id: "tablet", label: "Tablet 768", width: 768 },
  { id: "desktop", label: "Desktop 1280", width: 1280 },
];

export function browserViewportPreset(id: string): BrowserViewportPreset {
  return BROWSER_VIEWPORTS.find(preset => preset.id === id) ?? BROWSER_VIEWPORTS[0]!;
}

/**
 * Where the page sits in a slot `slotWidth` wide: a preset's width, centred,
 * or the whole slot when it fits none or the slot is narrower than the preset.
 */
export function browserViewportFrame(slotWidth: number, id: BrowserViewportId): { width: number; left: number } {
  const slot = Number.isFinite(slotWidth) && slotWidth > 0 ? Math.floor(slotWidth) : 0;
  const preset = browserViewportPreset(id).width;
  const width = preset === null ? slot : Math.min(preset, slot);
  return { width, left: Math.floor((slot - width) / 2) };
}
