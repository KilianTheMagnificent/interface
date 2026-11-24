/**
 * The renderer half of the voice assistant's UI tools.
 *
 * The host serves `app_state`, the panes and the sessions itself; the four
 * tools here are the ones that need the live document — what is on screen, and
 * driving it the way a person would. `App.tsx` owns the `window.iface` round
 * trip; everything below is a pure function of a Document, which is what makes
 * it testable without a browser.
 *
 * IDS ARE NEVER REUSED. `snapshot` mints `v1, v2, …` from a counter that only
 * ever goes up and rebuilds the live map, so an `act` against an id the newest
 * snapshot did not hand out is REFUSED rather than applied to whichever element
 * happens to hold that number now — the failure mode a per-snapshot counter
 * would produce silently, on the assistant's stalest information.
 */

import type { VoiceEvent } from "../shared/ipc";

/** The exact answer for an id the newest snapshot does not contain. */
export const STALE_ID_MESSAGE = "unknown id; call ui_snapshot again";

/**
 * The exact answer for a target inside a guarded region. The assistant may SEE
 * an approval — the snapshot reports those elements, marked `guarded` — but the
 * click that grants a permission is the user's alone. Without this an allowed
 * `ui_act` would let a voice-committed turn approve its own `send_to_pane`,
 * which is the one thing the design says never happens.
 */
export const GUARDED_MESSAGE = "refused: approval controls are reserved for the user";

/** The mark a component puts on the container that holds approval controls. */
export const GUARD_ATTR = "data-voice-guard";

/** The mark a component puts on secret or bearer-code subtrees snapshot must omit. */
export const REDACT_ATTR = "data-voice-redact";

/** The `ui_act` success body. The host wraps it as text content verbatim. */
export const ACT_OK = '{"ok":true}';

const MAX_SNAPSHOT_LINES = 400;
const TERMINAL_TAIL_LINES = 30;
const MAX_LABEL_CHARS = 80;

export type VoiceAction = "click" | "type" | "key" | "focus";

export interface VoiceActResult {
  ok: boolean;
  message?: string;
}

export interface VoiceUiOutcome {
  ok: boolean;
  output: string;
}

let nextVoiceId = 1;
let live = new Map<string, Element>();
/** Ids the newest snapshot found under a `GUARD_ATTR` container. */
let guarded = new Set<string>();

function collapse(text: string): string {
  const flat = text.replace(/\s+/gu, " ").trim();
  return flat.length > MAX_LABEL_CHARS ? `${flat.slice(0, MAX_LABEL_CHARS - 1)}…` : flat;
}

function hasClass(element: Element, name: string): boolean {
  const classes = element.getAttribute("class");
  return classes !== null && classes.split(/\s+/u).includes(name);
}

function childElements(element: Element): readonly Element[] {
  return Array.from(element.children);
}

/**
 * Skipped whole, subtree and all. These are the three ways this app takes a
 * region out of the accessibility tree — the shell does it to everything behind
 * an open dialog — and a snapshot that reported them would be describing a
 * surface the user cannot reach.
 */
function isHidden(element: Element): boolean {
  return element.hasAttribute("hidden")
    || element.hasAttribute("inert")
    || element.getAttribute("aria-hidden") === "true";
}

function roleOf(element: Element): string | null {
  if (hasClass(element, "terminal-view")) return "terminal";
  const explicit = element.getAttribute("role");
  if (explicit !== null && explicit !== "") return explicit;
  switch (element.tagName) {
    case "BUTTON":
    case "SUMMARY":
      return "button";
    case "A":
      return element.hasAttribute("href") ? "link" : null;
    case "SELECT":
      return "combobox";
    case "TEXTAREA":
      return "textbox";
    case "INPUT": {
      const type = (element.getAttribute("type") ?? "text").toLowerCase();
      if (type === "hidden") return null;
      if (type === "checkbox") return "checkbox";
      if (type === "radio") return "radio";
      if (type === "button" || type === "submit" || type === "reset") return "button";
      return "textbox";
    }
    default:
      return null;
  }
}

function isPasswordInput(element: Element): boolean {
  return element.tagName === "INPUT"
    && (element.getAttribute("type") ?? "").toLowerCase() === "password";
}

function hasRedactedDescendant(element: Element): boolean {
  for (const child of childElements(element)) {
    if (child.hasAttribute(REDACT_ATTR) || hasRedactedDescendant(child)) return true;
  }
  return false;
}

function unredactedText(element: Element): string {
  if (element.hasAttribute(REDACT_ATTR)) return "";
  if (!hasRedactedDescendant(element)) return element.textContent ?? "";
  const nodes = (element as { childNodes?: ArrayLike<{ nodeType: number; nodeValue: string | null }> }).childNodes;
  if (nodes !== undefined) {
    return Array.from(nodes).map(node => {
      if (node.nodeType === 3) return node.nodeValue ?? "";
      if (node.nodeType === 1) return unredactedText(node as unknown as Element);
      return "";
    }).join("");
  }
  const maybeOwn = (element as { own?: unknown }).own;
  const own = typeof maybeOwn === "string" ? maybeOwn : "";
  return [own, ...childElements(element).map(unredactedText)].join(" ");
}

function labelOf(element: Element, role: string, inRedact: boolean): string {
  const aria = collapse(element.getAttribute("aria-label") ?? "");
  if (aria !== "") return aria;
  if (role === "textbox" || role === "combobox") {
    const placeholder = collapse(element.getAttribute("placeholder") ?? "");
    if (placeholder !== "") return placeholder;
  }
  if (inRedact) return "";
  const text = collapse(unredactedText(element));
  if (text !== "") return text;
  return collapse(element.getAttribute("title") ?? "");
}

function statesOf(element: Element): readonly string[] {
  const states: string[] = [];
  if (element.getAttribute("aria-pressed") === "true") states.push("pressed");
  if (element.getAttribute("aria-selected") === "true") states.push("selected");
  const expanded = element.getAttribute("aria-expanded");
  if (expanded === "true") states.push("expanded");
  else if (expanded === "false") states.push("collapsed");
  if (element.getAttribute("aria-checked") === "true") states.push("checked");
  const current = element.getAttribute("aria-current");
  if (current !== null && current !== "" && current !== "false") states.push("current");
  if (element.hasAttribute("disabled") || element.getAttribute("aria-disabled") === "true") {
    states.push("disabled");
  }
  if (element.getAttribute("aria-busy") === "true") states.push("busy");
  return states;
}

function firstWithClass(element: Element, name: string): Element | undefined {
  for (const child of childElements(element)) {
    if (hasClass(child, name)) return child;
    const found = firstWithClass(child, name);
    if (found !== undefined) return found;
  }
  return undefined;
}

/**
 * The visible tail of one terminal, read out of xterm's own rendered rows.
 * xterm keeps the scrollback in a JS buffer the DOM never shows, so this is
 * the screen, not the history — which is what "what is on my screen" means.
 */
function terminalTail(element: Element): readonly string[] {
  const rows = firstWithClass(element, "xterm-rows");
  if (rows === undefined) return [];
  const lines = childElements(rows).map(row => (row.textContent ?? "").trimEnd());
  while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines.slice(-TERMINAL_TAIL_LINES);
}

function describe(id: string, role: string, element: Element, depth: number,
    inGuard: boolean, inRedact: boolean): string {
  const parts = [`[${id}]`, role, JSON.stringify(labelOf(element, role, inRedact))];
  const value = (element as { value?: unknown }).value;
  if (typeof value === "string" && !inRedact && !isPasswordInput(element)) {
    parts.push(`value=${JSON.stringify(value)}`);
  }
  parts.push(...statesOf(element));
  // Reported, not hidden: an assistant that cannot see the approval it is
  // waiting on would keep asking why nothing moved.
  if (inGuard) parts.push("guarded");
  return `${"  ".repeat(depth)}${parts.join(" ")}`;
}

function visit(element: Element, depth: number, lines: string[], guard: boolean,
    redact: boolean): void {
  if (lines.length >= MAX_SNAPSHOT_LINES || isHidden(element)) return;
  const inGuard = guard || element.hasAttribute(GUARD_ATTR);
  const inRedact = redact || element.hasAttribute(REDACT_ATTR);
  const role = roleOf(element);
  if (role === null) {
    for (const child of childElements(element)) visit(child, depth, lines, inGuard, inRedact);
    return;
  }
  const id = `v${String(nextVoiceId)}`;
  nextVoiceId += 1;
  element.setAttribute("data-voice-id", id);
  live.set(id, element);
  if (inGuard) guarded.add(id);
  lines.push(describe(id, role, element, depth, inGuard, inRedact));
  if (role === "terminal") {
    if (!inRedact) {
      // A terminal's content is its buffer, not its controls.
      const indent = `${"  ".repeat(depth)}    `;
      for (const text of terminalTail(element)) lines.push(indent + text);
    }
    return;
  }
  for (const child of childElements(element)) visit(child, depth + 1, lines, inGuard, inRedact);
}

/**
 * The reachable document as one indented line per element the assistant can
 * name, newest ids assigned as it goes. Indentation is depth among REPORTED
 * elements: this app nests twenty presentational divs between two buttons, and
 * raw DOM depth would spend the whole budget on leading spaces.
 */
export function snapshot(root: Document): string {
  live = new Map();
  guarded = new Set();
  const lines: string[] = [];
  const body: Element | null = root.body;
  if (body !== null) visit(body, 0, lines, false, false);
  if (lines.length >= MAX_SNAPSHOT_LINES) {
    lines.length = MAX_SNAPSHOT_LINES - 1;
    lines.push(`… truncated at ${String(MAX_SNAPSHOT_LINES)} lines`);
  }
  return lines.join("\n");
}

function focusOf(element: Element): void {
  (element as { focus?: () => void }).focus?.();
}

/**
 * React installs its own value tracker on the INSTANCE, so assigning `.value`
 * sets the field and leaves React believing nothing changed. Calling the
 * prototype's setter and dispatching `input` is the sequence React's own
 * change tracking recognises.
 */
function typeInto(element: Element, text: string): VoiceActResult {
  if (element.tagName !== "INPUT" && element.tagName !== "TEXTAREA") {
    return { ok: false, message: "type only works on an input or textarea" };
  }
  const proto: object | null = Object.getPrototypeOf(element) as object | null;
  const setter = proto === null
    ? undefined
    : Object.getOwnPropertyDescriptor(proto, "value")?.set;
  focusOf(element);
  if (setter === undefined) (element as { value?: string }).value = text;
  else setter.call(element, text);
  element.dispatchEvent(new Event("input", { bubbles: true }));
  return { ok: true };
}

function pressKey(element: Element, key: string): VoiceActResult {
  if (key === "") return { ok: false, message: "key needs the key name in text" };
  focusOf(element);
  element.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
  element.dispatchEvent(new KeyboardEvent("keyup", { key, bubbles: true }));
  return { ok: true };
}

/**
 * Guarded as of the newest snapshot, or guarded in the document right now: the
 * set answers for the walk that minted the id, and `closest` re-checks the live
 * tree in case the element moved under a guard since — a browser has that
 * method, the test fixture does not, and either answer alone refuses.
 */
function isGuarded(id: string, element: Element): boolean {
  if (guarded.has(id)) return true;
  const { closest } = element as { closest?: (selector: string) => Element | null };
  return typeof closest === "function" && closest.call(element, `[${GUARD_ATTR}]`) !== null;
}

/** Drive one element from the newest snapshot. Never throws. */
export function act(id: string, action: VoiceAction, text?: string): VoiceActResult {
  const element = live.get(id);
  if (element === undefined) return { ok: false, message: STALE_ID_MESSAGE };
  if (isGuarded(id, element)) return { ok: false, message: GUARDED_MESSAGE };
  try {
    switch (action) {
      case "click":
        focusOf(element);
        (element as { click?: () => void }).click?.();
        return { ok: true };
      case "focus":
        focusOf(element);
        return { ok: true };
      case "type":
        return typeInto(element, text ?? "");
      case "key":
        return pressKey(element, text ?? "");
      default:
        return { ok: false, message: `unknown action: ${String(action)}` };
    }
  } catch (error) {
    return { ok: false, message: error instanceof Error ? error.message : String(error) };
  }
}

function find(element: Element, match: (candidate: Element) => boolean): Element | undefined {
  for (const child of childElements(element)) {
    if (match(child)) return child;
    const found = find(child, match);
    if (found !== undefined) return found;
  }
  return undefined;
}

/**
 * Reveal one project, exactly as clicking its sidebar header does — except that
 * a header already open is left open. The sidebar's own click TOGGLES, and an
 * assistant asked to open a project must never close it instead.
 */
export function openProject(root: Document, path: string): VoiceActResult {
  const body: Element | null = root.body;
  const header = body === null ? undefined : find(body, candidate =>
    hasClass(candidate, "group-header") && candidate.getAttribute("title") === path);
  if (header === undefined) return { ok: false, message: `no project row for ${path}` };
  focusOf(header);
  if (header.getAttribute("aria-expanded") === "false") {
    (header as { click?: () => void }).click?.();
  }
  return { ok: true };
}

/** Bring one terminal pane forward, exactly as clicking its tab does. */
export function focusPane(root: Document, paneId: string): VoiceActResult {
  const body: Element | null = root.body;
  const tab = body === null ? undefined : find(body, candidate =>
    hasClass(candidate, "terminal-tab") && candidate.getAttribute("data-terminal-id") === paneId);
  if (tab === undefined) return { ok: false, message: `no pane ${paneId}` };
  focusOf(tab);
  (tab as { click?: () => void }).click?.();
  return { ok: true };
}

function field(input: unknown, name: string): string {
  if (typeof input !== "object" || input === null) return "";
  const value = (input as Record<string, unknown>)[name];
  return typeof value === "string" ? value : "";
}

/**
 * One host `voiceUiCall` executed against the live document. The output is the
 * tool's result text verbatim: a snapshot, `{"ok":true}`, or the reason it did
 * not happen.
 */
export function runUiCall(root: Document, tool: string, input: unknown): VoiceUiOutcome {
  switch (tool) {
    case "ui_snapshot":
      return { ok: true, output: snapshot(root) };
    case "ui_act": {
      const result = act(field(input, "id"), field(input, "action") as VoiceAction,
        field(input, "text"));
      return { ok: result.ok, output: result.ok ? ACT_OK : result.message ?? "" };
    }
    case "open_project": {
      const result = openProject(root, field(input, "root"));
      return { ok: result.ok, output: result.ok ? ACT_OK : result.message ?? "" };
    }
    case "focus_pane": {
      const result = focusPane(root, field(input, "pane_id"));
      return { ok: result.ok, output: result.ok ? ACT_OK : result.message ?? "" };
    }
    default:
      return { ok: false, output: `unknown voice UI tool: ${tool}` };
  }
}

/* ---- the indicator's own frame ------------------------------------------- */

/**
 * The frame the statusbar draws, from the frame the host just pushed.
 *
 * Every push carries the whole event, so a field the host has nothing new for
 * arrives null — and a listening turn pushes level frames at ~10 Hz between the
 * partials. Taking each null literally would blink the heard words out on every
 * level frame, empty the meter on every partial, and erase a `notify` message a
 * tenth of a second after it appeared. So within one state null means
 * unchanged; the state changing is what clears them, because the words heard in
 * the last utterance are not the words being heard now.
 */
export function mergeVoiceEvent(prev: VoiceEvent | null, event: VoiceEvent): VoiceEvent {
  const same = prev !== null && prev.state === event.state;
  return {
    ...event,
    partial: event.partial ?? (same ? prev.partial : null),
    level: event.level ?? (same ? prev.level : null),
    message: event.message ?? (same ? prev.message : null),
  };
}

export const VOICE_METER_BARS = [1, 2, 3, 4, 5] as const;

/**
 * Microphone level as lit bars. Half a bar is still a bar: at conversational
 * volume the raw RMS sits low, and a meter that reads empty while the user is
 * mid-sentence says "not hearing you" when the truth is the opposite.
 */
export function voiceMeterBars(level: number | null): number {
  if (level === null || !Number.isFinite(level) || level <= 0) return 0;
  return Math.min(VOICE_METER_BARS.length, Math.ceil(Math.min(1, level) * VOICE_METER_BARS.length));
}

/**
 * `mergeVoiceEvent`, but `prev` itself when the frame changes nothing drawn.
 * Level frames arrive every 100 ms and only the lit-bar count is shown, so
 * keeping the same state object lets React skip the whole App render.
 */
export function nextVoiceState(prev: VoiceEvent | null, event: VoiceEvent): VoiceEvent {
  const next = mergeVoiceEvent(prev, event);
  return prev !== null
    && next.state === prev.state && next.session_key === prev.session_key
    && next.cwd === prev.cwd && next.partial === prev.partial
    && next.message === prev.message && next.microphone === prev.microphone
    && (next.asset === prev.asset || (next.asset !== null && prev.asset !== null
      && next.asset.status === prev.asset.status && next.asset.progress === prev.asset.progress))
    && voiceMeterBars(next.level) === voiceMeterBars(prev.level)
    ? prev : next;
}
