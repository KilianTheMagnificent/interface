// A Browser tab that behaves like a Chrome tab, against real pages in real
// <webview>s (the browser-chrome lane). The lane's main process serves the
// pages, installs the app's own guards with test hooks (a temp Downloads
// folder, a scripted answer for the permission sheet) and drives the pages;
// these helpers render the real terminal centre with one pane holding a
// terminal and a Browser tab, pass main's Browser events to it as App does,
// and report what the pane shows. Test code only.
import { createRef } from "react";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import { parseBrowserEvent } from "../../shared/browser-events";
import {
  IntegratedTerminalCenter, type IntegratedTerminalCenterHandle, type TerminalRendererApi,
} from "../components/IntegratedTerminalCenter";
import type { TerminalMenuModel, TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, newBrowserTabId, openBrowserTabInPane, openTerminalTab,
  serializeTerminalWorkspaceState, setBrowserTabUrl,
} from "../terminal-workspace";
import { assert, frames, noop, until } from "./fixture-support";

interface WebviewProbe extends HTMLElement {
  getWebContentsId(): number;
  getURL(): string;
}

let unsubscribe: (() => void) | null = null;
let menu: TerminalMenuModel | null = null;

const pane = (): HTMLElement => {
  const found = document.querySelector<HTMLElement>(".browser-chrome-fixture .terminal-pane");
  assert(found, "The pane is not rendered");
  return found;
};

/** The centre with one pane: a shell and a Browser tab on `url`. Resolves once the page is attached. */
export async function browserChromeFixture(root: Root, url: string): Promise<unknown> {
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  const stamp = "2026-11-15T00:00:00Z";
  const terminal: TerminalSummary = {
    id: "00000000-0000-4000-8000-000000000b01", generation: 1, title: "zsh", root: "/fixture/browser",
    cwd: "/fixture/browser", state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "shell" }, createdAt: stamp, updatedAt: stamp,
  };
  const tabId = newBrowserTabId(() => "chrome-fixture-page");
  const layout = setBrowserTabUrl(openBrowserTabInPane(
    openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "chrome-pane", terminal.id), "chrome-pane", tabId, "unused"),
  tabId, url);
  const record: TerminalWorkspaceRecord = {
    id: "browser-chrome-workspace", name: "browser", createdAt: stamp, updatedAt: stamp,
    terminals: [terminal], layout: serializeTerminalWorkspaceState(layout),
  };
  const unexpected = (): never => { throw new Error("The Browser fixture performed a terminal operation"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals: [terminal] }),
    listTerminalLaunchProfiles: async () => [{ id: "shell", label: "Shell", available: true }],
    getTerminalWorkspaces: async () => [record], setTerminalWorkspaces: async value => value,
    onTerminalData: () => noop, onTerminalLifecycle: () => noop, onTerminalDataFailure: () => noop,
    sendTerminalInput: noop,
    resizeTerminal: async request => ({ ...terminal, size: request.size }),
    createTerminal: unexpected, closeTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const center = createRef<IntegratedTerminalCenterHandle>();
  let switcher: TerminalWorkspaceSwitcher | null = null;
  const errors: string[] = [];
  // As App does: main's Browser events, parsed, to the centre.
  unsubscribe = window.iface.onBrowserEvent(raw => {
    const event = parseBrowserEvent(raw);
    if (event !== null) center.current?.browserEvent(event);
  });
  flushSync(() => root.render(<div className="browser-chrome-fixture" style={{ height: "100vh", display: "flex" }}>
    <IntegratedTerminalCenter ref={center} api={api} activeCwd="/fixture/browser" visible
      onSwitcherChange={value => { switcher = value; }} onMenuChange={value => { menu = value; }}
      onError={message => errors.push(message)} />
  </div>));
  await until(() => switcher !== null, "The workspace did not load", { timeoutMs: 8000 });
  flushSync(() => switcher!.onSelect(record.id));
  await until(() => document.querySelector(".browser-chrome-fixture .browser-page-view") !== null,
    () => `The Browser tab made no page: ${errors.join("; ")}`, { timeoutMs: 8000 });
  await frames();
  return { errors };
}

/** What the pane shows: its tabs in order, its pages, the find field and the pane's notices. */
export async function browserChromeState(): Promise<unknown> {
  await frames();
  const tabs = [...pane().querySelectorAll<HTMLElement>(".terminal-tab")].map(tab => ({
    id: tab.dataset.terminalId ?? "", active: tab.getAttribute("aria-selected") === "true",
  }));
  const pages = [...document.querySelectorAll<HTMLElement>(".browser-chrome-fixture .browser-page")].map(host => {
    const view = host.querySelector<WebviewProbe>(".browser-page-view");
    let id: number | null = null;
    let url: string | null = null;
    try { id = view?.getWebContentsId() ?? null; url = view?.getURL() ?? null; } catch { id = null; }
    const box = host.getBoundingClientRect();
    return { id, url, parked: host.classList.contains("is-parked"), fullscreen: host.classList.contains("is-fullscreen"),
      box: { x: box.x, y: box.y, width: box.width, height: box.height } };
  });
  const find = document.querySelector<HTMLElement>(".browser-page .browser-find");
  const shown = pages.find(page => !page.parked);
  const slot = pane().querySelector<HTMLElement>(".terminal-tabpanel.is-active .browser-pane-slot");
  const slotBox = slot?.getBoundingClientRect();
  return {
    panes: document.querySelectorAll(".browser-chrome-fixture .terminal-pane").length,
    tabs, pages, shownPage: shown?.id ?? null,
    find: find === null ? null : {
      status: find.querySelector(".terminal-search-result")?.textContent ?? "",
      focused: find.contains(document.activeElement),
    },
    hints: [...pane().querySelectorAll(".terminal-tabpanel.is-active .browser-pane-hint")].map(line => line.textContent),
    addressFocused: document.activeElement?.getAttribute("aria-label") === "Address",
    slot: slotBox === undefined ? null : { x: slotBox.x, y: slotBox.y, width: slotBox.width, height: slotBox.height },
    window: { width: innerWidth, height: innerHeight },
    errors: [...document.querySelectorAll(".terminal-center-error, [role='alert']")].map(node => node.textContent),
  };
}

/** Type into the find field as a person does. */
export async function browserChromeFindType(text: string): Promise<void> {
  const input = document.querySelector<HTMLInputElement>(".browser-page .browser-find input");
  assert(input, "The find field is not open");
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
  flushSync(() => input.dispatchEvent(new Event("input", { bubbles: true })));
  await frames();
}

/** A key in the find field or the active tab's address bar. */
export async function browserChromeKey(target: "find" | "address", key: string,
    modifiers: { meta?: boolean; shift?: boolean } = {}): Promise<void> {
  const element = target === "find"
    ? document.querySelector<HTMLInputElement>(".browser-page .browser-find input")
    : pane().querySelector<HTMLInputElement>(".terminal-tabpanel.is-active [aria-label='Address']");
  assert(element, `No ${target} field`);
  element.focus();
  flushSync(() => element.dispatchEvent(new KeyboardEvent("keydown", {
    key, bubbles: true, cancelable: true, metaKey: modifiers.meta === true, shiftKey: modifiers.shift === true,
  })));
  await frames();
}

/** Show a tab by its id. */
export async function browserChromeSelect(id: string): Promise<void> {
  const tab = pane().querySelector<HTMLButtonElement>(`.terminal-tab[data-terminal-id="${id}"]`);
  assert(tab, `No tab ${id}`);
  flushSync(() => tab.click());
  await frames();
}

/** Close a tab by its id with its own ×. */
export async function browserChromeClose(id: string): Promise<void> {
  const close = pane().querySelector<HTMLElement>(`.terminal-tab[data-terminal-id="${id}"]`)
    ?.closest(".terminal-tab-wrap")?.querySelector<HTMLButtonElement>(".terminal-tab-close");
  assert(close, `No close button for tab ${id}`);
  flushSync(() => close.click());
  await frames();
}

/** Flip the panes to cards, or back, as the ··· menu's Flip does; resolves with the view shown. */
export async function browserChromeFlip(): Promise<string> {
  assert(menu, "The centre published no layout menu");
  flushSync(() => menu!.onFlip());
  await frames();
  return menu.view;
}

export function clearBrowserChromeFixture(root: Root): void {
  unsubscribe?.();
  unsubscribe = null;
  flushSync(() => root.render(null));
}
