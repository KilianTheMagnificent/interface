/**
 * The Browser pane's picker as it runs INSIDE the page: the embedder sends it
 * with `<webview>.executeJavaScript`, so it runs in the page's main world, with
 * no preload, no bridge and no channel back but its own return value.
 *
 * Each function below is sent as its source text (`browserPointScript`), so
 * each must be self-contained: no imports, no module constants, nothing from
 * an outer scope; what it needs arrives as an argument. The page can see and
 * tamper with all of it, which is why the embedder treats the result as
 * untrusted (`browserPickFromPage` in `browser-point.ts`).
 *
 * While picking, a full-page layer (a closed shadow root) takes every pointer
 * event, so a click never reaches the page's links or buttons; Escape cancels.
 * The layer and its listeners are removed on a pick, a cancel, a navigation
 * (the document goes) or the embedder's cancel script.
 */

export type BrowserPointMode = "element" | "region";

/** Arguments every in-page function receives. */
export interface BrowserPointOptions {
  /** `Symbol.for` key the running picker's stop function is kept under. */
  readonly key: string;
  readonly selector: number;
  /** How much text the page side reads before the embedder bounds it again. */
  readonly text: number;
  /** What to do, shown at the foot of the page while picking. */
  readonly hint: string;
}

const KEY = "interface.browser-point";
const HINTS: Readonly<Record<BrowserPointMode, string>> = {
  element: "Click an element to comment on it · Esc cancels",
  region: "Drag over the part of the page to capture · Esc cancels",
};

/**
 * A short, stable CSS selector for `element` in its document, or null: an id
 * when it is unique, else tag, up to two stable class names and
 * `:nth-of-type` per level, climbing only until the path is unique. Ids and
 * classes that look generated (long digit runs, CSS-in-JS prefixes) are
 * skipped, so the selector survives a rebuild. Runs in the page.
 */
export function buildSelector(element: Element, limit: number): string | null {
  const doc = element.ownerDocument;
  const escape = (value: string): string => typeof CSS !== "undefined" && typeof CSS.escape === "function"
    ? CSS.escape(value) : value.replace(/[^a-zA-Z0-9_-]/g, character => `\\${character}`);
  const unique = (selector: string): boolean => {
    try {
      const found = doc.querySelectorAll(selector);
      return found.length === 1 && found[0] === element;
    } catch { return false; }
  };
  const stableId = (id: string): boolean => id.length > 0 && id.length <= 64 && /^[A-Za-z][\w-]*$/.test(id)
    && !/\d{4,}/.test(id);
  const stableClass = (name: string): boolean => name.length <= 32 && /^[A-Za-z_-][\w-]*$/.test(name)
    && !/\d{3,}/.test(name) && !/^(?:css|sc|jsx|svelte|emotion|ng-tns)-/.test(name);
  const parts: string[] = [];
  let node: Element | null = element;
  while (node !== null) {
    const tag = /^[a-z][a-z0-9-]*$/.test(node.localName) ? node.localName : "*";
    if (stableId(node.id)) {
      const path = [`#${escape(node.id)}`, ...parts].join(" > ");
      if (path.length > limit) return null;
      if (unique(path)) return path;
    }
    let part = tag + Array.from(node.classList).filter(stableClass).slice(0, 2)
      .map(name => `.${escape(name)}`).join("");
    const parent: Element | null = node.parentElement;
    if (parent !== null) {
      const localName = node.localName;
      const same = Array.from(parent.children).filter(child => child.localName === localName);
      if (same.length > 1) part += `:nth-of-type(${(same.indexOf(node) + 1).toString()})`;
    }
    parts.unshift(part);
    const path = parts.join(" > ");
    if (path.length > limit) return null;
    if (unique(path)) return path;
    node = parent;
  }
  return null;
}

/**
 * What the page says about `element`: tag, visible text, accessible name and
 * role, its box and the view's size. Never a form field's value. Runs in the page.
 */
export function describeElement(element: Element, textLimit: number): Record<string, unknown> {
  const clip = (value: unknown): string => typeof value === "string" ? value.replace(/\s+/g, " ").trim().slice(0, textLimit) : "";
  const tag = element.localName;
  const html = element as HTMLElement;
  const text = clip(typeof html.innerText === "string" ? html.innerText : element.textContent);
  const attr = (name: string): string => clip(element.getAttribute(name));
  const implicit: Record<string, string> = {
    button: "button", nav: "navigation", main: "main", header: "banner", footer: "contentinfo", form: "form",
    img: "img", select: "combobox", textarea: "textbox", ul: "list", ol: "list", li: "listitem", table: "table",
    h1: "heading", h2: "heading", h3: "heading", h4: "heading", h5: "heading", h6: "heading", dialog: "dialog",
  };
  let role = attr("role").split(" ")[0] ?? "";
  if (role === "" && tag === "a" && element.hasAttribute("href")) role = "link";
  if (role === "" && tag === "input") {
    const type = attr("type").toLowerCase();
    role = type === "checkbox" ? "checkbox" : type === "radio" ? "radio"
      : type === "button" || type === "submit" || type === "reset" ? "button" : "textbox";
  }
  if (role === "") role = implicit[tag] ?? "";
  let name = attr("aria-label");
  if (name === "") {
    const ids = attr("aria-labelledby").split(" ").filter(Boolean).slice(0, 4);
    name = clip(ids.map(id => element.ownerDocument.getElementById(id)?.textContent ?? "").join(" "));
  }
  if (name === "") name = attr("alt") || attr("title") || attr("placeholder");
  const labels = (element as HTMLInputElement).labels;
  if (name === "" && labels && labels.length > 0) name = clip(labels[0]?.textContent);
  if (name === "" && (role === "button" || role === "link" || role === "heading")) name = text;
  const box = element.getBoundingClientRect();
  return {
    kind: "element", tag, text, name, role,
    frame: tag === "iframe" || tag === "frame" || tag === "object" || tag === "embed",
    box: { x: box.x, y: box.y, width: box.width, height: box.height },
    viewport: { width: window.innerWidth, height: window.innerHeight },
  };
}

/**
 * The picker itself. Resolves with a raw pick, or null on Escape or a
 * cancel, after its layer is gone from the page and two frames have
 * painted, so a picture taken next never shows the highlight. Runs in the page.
 */
export function pagePoint(mode: BrowserPointMode, options: BrowserPointOptions,
    select: (element: Element, limit: number) => string | null,
    describe: (element: Element, textLimit: number) => Record<string, unknown>): Promise<unknown> {
  const key = Symbol.for(options.key);
  const slots = window as unknown as Record<symbol, unknown>;
  const earlier = slots[key];
  if (typeof earlier === "function") (earlier as () => void)();
  return new Promise(resolve => {
    const doc = document;
    const host = doc.createElement("div");
    const root = host.attachShadow({ mode: "closed" });
    const mark = doc.createElement("div");
    const label = doc.createElement("div");
    const hint = doc.createElement("div");
    const pin = (target: HTMLElement, styles: Record<string, string>): void => {
      for (const [name, value] of Object.entries(styles)) target.style.setProperty(name, value, "important");
    };
    pin(host, { all: "initial", position: "fixed", inset: "0", "z-index": "2147483647", display: "block",
      cursor: "crosshair", background: "transparent", "pointer-events": "auto" });
    pin(mark, { position: "fixed", display: "none", "box-sizing": "border-box", "pointer-events": "none",
      border: "2px solid #1f6feb", background: "rgba(31, 111, 235, 0.16)", "border-radius": "2px" });
    pin(label, { position: "fixed", display: "none", "pointer-events": "none", padding: "1px 6px",
      font: "11px/16px -apple-system, system-ui, sans-serif", color: "#ffffff", background: "#1f6feb",
      "border-radius": "3px", "white-space": "nowrap" });
    // At the foot of the page, so picking moves nothing under the pointer.
    pin(hint, { position: "fixed", left: "50%", bottom: "12px", transform: "translateX(-50%)", "pointer-events": "none",
      padding: "4px 10px", font: "12px/18px -apple-system, system-ui, sans-serif", color: "#ffffff",
      background: "rgba(20, 24, 31, 0.86)", "border-radius": "6px", "white-space": "nowrap" });
    hint.textContent = options.hint;
    host.setAttribute("data-interface-point", mode);
    root.append(mark, label, hint);
    doc.documentElement.append(host);

    let done = false;
    let pointer: { x: number; y: number } | null = null;
    let start: { x: number; y: number } | null = null;
    const block = (event: Event): void => {
      event.preventDefault(); event.stopPropagation(); event.stopImmediatePropagation();
    };
    const show = (box: { x: number; y: number; width: number; height: number }, text: string): void => {
      pin(mark, { display: "block", left: `${box.x}px`, top: `${box.y}px`, width: `${box.width}px`, height: `${box.height}px` });
      label.textContent = text;
      pin(label, { display: "block", left: `${Math.max(0, box.x)}px`,
        top: `${box.y >= 20 ? box.y - 19 : box.y + box.height + 3}px` });
    };
    const hide = (): void => {
      pin(mark, { display: "none" }); pin(label, { display: "none" });
      host.removeAttribute("data-highlight");
    };
    const under = (x: number, y: number): Element | null =>
      doc.elementsFromPoint(x, y).find(element => element !== host) ?? null;
    const highlight = (): void => {
      const target = pointer === null ? null : under(pointer.x, pointer.y);
      if (target === null) { hide(); return; }
      const box = target.getBoundingClientRect();
      show(box, `${target.localName} ${Math.round(box.width)}×${Math.round(box.height)}`);
      host.setAttribute("data-highlight", target.localName);
    };
    const span = (x: number, y: number): { x: number; y: number; width: number; height: number } => {
      const from = start ?? { x, y };
      return { x: Math.min(from.x, x), y: Math.min(from.y, y), width: Math.abs(x - from.x), height: Math.abs(y - from.y) };
    };
    const viewport = (): { width: number; height: number } => ({ width: window.innerWidth, height: window.innerHeight });

    const onMove = (event: PointerEvent): void => {
      block(event);
      pointer = { x: event.clientX, y: event.clientY };
      if (mode === "element") { highlight(); return; }
      if (start === null) return;
      const box = span(event.clientX, event.clientY);
      show(box, `${Math.round(box.width)}×${Math.round(box.height)}`);
      host.setAttribute("data-highlight", "region");
    };
    const onDown = (event: PointerEvent): void => {
      block(event);
      if (mode !== "region" || event.button !== 0) return;
      start = { x: event.clientX, y: event.clientY };
      try { host.setPointerCapture(event.pointerId); } catch { /* the region still follows moves over the layer */ }
    };
    const onUp = (event: PointerEvent): void => {
      block(event);
      if (mode !== "region" || start === null) return;
      const box = span(event.clientX, event.clientY);
      start = null;
      if (box.width < 4 || box.height < 4) { hide(); return; }
      finish({ kind: "region", box, viewport: viewport() });
    };
    const onClick = (event: MouseEvent): void => {
      block(event);
      if (mode !== "element") return;
      const target = under(event.clientX, event.clientY);
      if (target === null) return;
      finish({ ...describe(target, options.text), selector: select(target, options.selector) });
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      block(event);
      finish(null);
    };
    const onScroll = (): void => { if (mode === "element") highlight(); else if (start === null) hide(); };
    const quiet = ["mousedown", "mouseup", "dblclick", "contextmenu", "auxclick", "touchstart", "touchend", "wheel"];

    function cleanup(): void {
      host.removeEventListener("pointermove", onMove);
      host.removeEventListener("pointerdown", onDown);
      host.removeEventListener("pointerup", onUp);
      host.removeEventListener("click", onClick);
      for (const type of quiet) host.removeEventListener(type, block);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", onScroll, true);
      host.remove();
      if (slots[key] === stop) Reflect.deleteProperty(slots, key);
    }
    function finish(value: unknown): void {
      if (done) return;
      done = true;
      cleanup();
      requestAnimationFrame(() => { requestAnimationFrame(() => { resolve(value); }); });
    }
    function stop(): void { finish(null); }

    host.addEventListener("pointermove", onMove);
    host.addEventListener("pointerdown", onDown);
    host.addEventListener("pointerup", onUp);
    host.addEventListener("click", onClick);
    for (const type of quiet) host.addEventListener(type, block, { passive: false });
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", onScroll, true);
    slots[key] = stop;
  });
}

/** The script that starts the picker in a page; its promise is the raw pick. */
export function browserPointScript(mode: BrowserPointMode): string {
  const options: BrowserPointOptions = { key: KEY, selector: 300, text: 400, hint: HINTS[mode] };
  return `(${pagePoint.toString()})(${JSON.stringify(mode)}, ${JSON.stringify(options)}, `
    + `${buildSelector.toString()}, ${describeElement.toString()})`;
}

/** Stops a running picker in the page, which removes its layer and listeners. */
export const BROWSER_POINT_CANCEL_SCRIPT = `(() => { const stop = window[Symbol.for(${JSON.stringify(KEY)})];`
  + ` if (typeof stop === "function") stop(); return true; })()`;
