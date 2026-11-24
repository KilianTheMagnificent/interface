// Point and fix in the Browser pane against a real page in a real <webview>
// (the browser-point lane). The lane's main process serves the page, pins the
// guest with the app's own guards and drives the pointer; these helpers render
// the agent overview's Browser column, press its controls and report what the
// pane and the page show. Test code only.
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { BrowserColumn, type BrowserAddToDraft } from "../components/BrowserPane";
import { addBrowserNoteToDraft, type BrowserNoteTarget } from "../browser-point";
import { buildSelector } from "../browser-point-page";
import { createDraftStore } from "../draft-store";
import { attachmentContextItems } from "../prompt-attach";
import type { PromptContextItem } from "../prompt-context";
import { assert, frames, until } from "./fixture-support";

const TARGETS: readonly BrowserNoteTarget[] = [
  { key: "claude:fixture-header", title: "Fix the settings header" },
  { key: "codex:fixture-chat", title: "No-project chat" },
];

interface PointFixture {
  readonly drafts: ReturnType<typeof createDraftStore>;
  readonly attachments: Map<string, PromptContextItem[]>;
  readonly images: Uint8Array[];
  host: HTMLElement | null;
}

let current: PointFixture | null = null;

const pane = (): HTMLElement => {
  const found = document.querySelector<HTMLElement>(".browser-point-fixture .browser-pane");
  assert(found, "The Browser pane is not rendered");
  return found;
};

const view = (): HTMLElement & { executeJavaScript(code: string): Promise<unknown> } => {
  const found = document.querySelector(".browser-point-fixture .browser-page-view");
  assert(found, "The Browser page has no <webview>");
  return found as HTMLElement & { executeJavaScript(code: string): Promise<unknown> };
};

/** Selector rules on this document: unique, short, stable, bounded. */
function selectorChecks(): Record<string, string | null> {
  const box = document.createElement("div");
  box.className = "selector-fixture";
  // Ids that look generated, classes from CSS-in-JS and a huge class list.
  const huge = `c${"x".repeat(5000)}`;
  box.innerHTML = `<section id="r123456"><ul class="menu css-1x2y3z"><li><a class="item">One</a></li>
    <li><a class="item sc-abc">Two</a></li></ul><p id="dup">a</p><p id="dup">b</p>
    <div class="${huge}"><span>deep</span></div><button id="go">Go</button><i id="weird:id">w</i></section>`;
  document.body.append(box);
  try {
    const two = box.querySelectorAll("a.item")[1]!;
    const dup = box.querySelectorAll("#dup")[1]!;
    const results = {
      id: buildSelector(box.querySelector("#go")!, 300),
      nth: buildSelector(two, 300),
      duplicateId: buildSelector(dup, 300),
      hugeClass: buildSelector(box.querySelector("span")!, 300),
      generatedId: buildSelector(box.querySelector("section")!, 300),
      oddId: buildSelector(box.querySelector("i")!, 300),
      tooLong: buildSelector(two, 10),
    };
    assert(results.id === "#go", `An element with a unique id was not named by it: ${String(results.id)}`);
    for (const [name, selector] of Object.entries(results)) {
      if (name === "tooLong") continue;
      assert(selector !== null && selector.length <= 300, `${name}: no bounded selector (${String(selector)})`);
      assert(document.querySelectorAll(selector).length === 1, `${name}: ${selector} is not unique`);
    }
    assert(document.querySelector(results.nth!) === two && results.nth!.includes(":nth-of-type(2)")
      && !results.nth!.includes("sc-abc") && !results.nth!.includes("css-1x2y3z"),
    `The second link's selector kept generated classes or lost its position: ${String(results.nth)}`);
    assert(document.querySelector(results.duplicateId!) === dup, `A duplicated id named the wrong element: ${String(results.duplicateId)}`);
    assert(!results.hugeClass!.includes("xxxx"), "A huge class name went into a selector");
    assert(!results.generatedId!.includes("r123456"), "A generated id went into a selector");
    assert(results.tooLong === null, "A selector over its limit was kept");
    return results;
  } finally { box.remove(); }
}

/** Renders the column's Browser on `url` and waits for the page to load. */
export async function browserPointFixture(root: Root, url: string): Promise<unknown> {
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  const selectors = selectorChecks();
  const fixture: PointFixture = { drafts: createDraftStore(), attachments: new Map(), images: [], host: null };
  current = fixture;
  const addToDraft: BrowserAddToDraft = (key, note, png) => addBrowserNoteToDraft({
    getDraft: target => fixture.drafts.get(target),
    setDraft: (target, text) => { fixture.drafts.set(target, text); },
    canAttach: target => target.startsWith("claude:"),
    attachImage: async (target, bytes) => {
      fixture.images.push(bytes);
      const path = `/fixture/images/browser-${String(fixture.images.length)}.png`;
      const blob = new Blob([new Uint8Array(bytes)], { type: "image/png" });
      const preview = await new Promise<string>(resolve => {
        const reader = new FileReader();
        reader.onload = () => { resolve(String(reader.result)); };
        reader.readAsDataURL(blob);
      });
      const items = await attachmentContextItems({
        readThumbnail: () => Promise.resolve(preview), mintId: kind => `${kind}-${String(fixture.images.length)}`,
        leaf: value => value.split("/").at(-1) ?? value,
      }, [{ path, kind: "file" }]);
      fixture.attachments.set(target, [...fixture.attachments.get(target) ?? [], ...items]);
      return [path];
    },
  }, key, note, png);
  flushSync(() => root.render(
    <div className="browser-point-fixture" style={{ position: "relative", display: "flex", flexDirection: "column",
      width: "100%", height: "100vh", background: "var(--bg)" }}>
      <style>{APP_SELECT_CSS}</style>
      <BrowserColumn url={url} active servers={[]} onFindServers={() => undefined} onNavigated={() => undefined}
        pointTargets={TARGETS} onAddToDraft={addToDraft} />
    </div>,
  ));
  await until(() => document.querySelector(".browser-point-fixture .browser-page-view") !== null,
    "The Browser never made its page");
  const deadline = performance.now() + 15_000;
  for (;;) {
    // Throws until the guest's first dom-ready.
    const ready = await Promise.resolve().then(() => view().executeJavaScript(
      "document.readyState === 'complete' && document.title")).catch(() => false);
    if (ready === "Point fixture") break;
    assert(performance.now() < deadline, `The fixture page did not load: ${String(ready)}`);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  await frames(3);
  return { view: rect(view()), selectors };
}

function rect(element: Element): { x: number; y: number; width: number; height: number } {
  const box = element.getBoundingClientRect();
  return { x: box.x, y: box.y, width: box.width, height: box.height };
}

/** Runs `code` in the page and returns its answer. */
export function browserPointGuest(code: string): Promise<unknown> {
  return view().executeJavaScript(code);
}

/** Presses a control in the pane by its text or label. */
export async function browserPointPress(name: string): Promise<void> {
  const button = [...pane().querySelectorAll<HTMLButtonElement>("button")]
    .find(candidate => candidate.textContent?.trim() === name || candidate.getAttribute("aria-label") === name);
  assert(button && !button.disabled, `No enabled "${name}" control in the Browser pane`);
  flushSync(() => { button.click(); });
  await frames(2);
}

/** Chooses `option` in the pane's select labelled `label`. */
export async function browserPointChoose(label: string, option: string): Promise<void> {
  const trigger = [...pane().querySelectorAll<HTMLButtonElement>(".app-select-trigger")]
    .find(candidate => candidate.getAttribute("aria-label")?.startsWith(label) === true);
  assert(trigger, `No "${label}" select in the Browser pane`);
  flushSync(() => { trigger.click(); });
  await until(() => [...document.querySelectorAll<HTMLElement>("[role='option']")]
    .some(row => row.textContent?.includes(option) === true), `"${label}" did not offer ${option}`);
  const row = [...document.querySelectorAll<HTMLElement>("[role='option']")].find(item => item.textContent?.includes(option) === true)!;
  flushSync(() => { row.click(); });
  await frames(2);
}

export async function browserPointComment(text: string): Promise<void> {
  const box = pane().querySelector<HTMLTextAreaElement>("textarea[aria-label='Comment on the capture']");
  assert(box, "The capture card has no comment field");
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  setter.call(box, text);
  box.dispatchEvent(new Event("input", { bubbles: true }));
  await frames(2);
}

/** What the pane shows now, and where notes and pictures went. */
export async function browserPointState(): Promise<unknown> {
  const fixture = current;
  assert(fixture, "The browser point fixture is not rendered");
  const card = pane().querySelector<HTMLElement>(".browser-point-card");
  const thumb = card?.querySelector<HTMLImageElement>(".browser-point-thumb") ?? null;
  if (thumb !== null && !thumb.complete) await thumb.decode().catch(() => undefined);
  const pressed = Object.fromEntries([...pane().querySelectorAll<HTMLButtonElement>(".browser-pane-point")]
    .map(button => [button.textContent?.trim() ?? "", button.getAttribute("aria-pressed") === "true"]));
  const sizes = await Promise.all(fixture.images.map(async bytes => {
    const bitmap = await createImageBitmap(new Blob([new Uint8Array(bytes)], { type: "image/png" }));
    const size = { width: bitmap.width, height: bitmap.height };
    bitmap.close();
    return size;
  }));
  return {
    pressed,
    hint: [...pane().querySelectorAll(".browser-pane-hint")].map(line => line.textContent ?? ""),
    card: card === null ? null : {
      summary: card.querySelector(".browser-point-summary")?.textContent ?? "",
      selector: card.querySelector(".browser-point-selector")?.textContent ?? null,
      thumb: thumb === null ? null : { width: thumb.naturalWidth, height: thumb.naturalHeight },
      // Page text is drawn as text: no element of the page's making is in the card.
      elements: [...card.querySelectorAll("*")].map(element => element.localName)
        .filter(name => !["img", "div", "p", "code", "textarea", "button", "span"].includes(name)),
      choices: [...card.querySelectorAll(".browser-pane-choice")].map(button => button.textContent ?? ""),
      addEnabled: card.querySelector<HTMLButtonElement>(".browser-point-add")?.disabled === false,
    },
    drafts: Object.fromEntries(TARGETS.map(target => [target.key, fixture.drafts.get(target.key)])),
    attachments: Object.fromEntries([...fixture.attachments].map(([key, items]) => [key, items.map(item => item.label)])),
    images: sizes,
    viewport: document.querySelector(".browser-point-fixture .browser-page")?.hasAttribute("data-viewport") ?? false,
    view: rect(view()),
  };
}

export function clearBrowserPointFixture(root: Root): void {
  flushSync(() => root.render(null));
  current = null;
}
