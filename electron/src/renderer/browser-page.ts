import {
  BROWSER_HOME_URL, BROWSER_PANE_PARTITION, browserHostNotFound, browserLoadError, browserPageUrl,
} from "./browser-pane";
import {
  browserCapture, browserCropRect, browserPickFromPage, browserViewportFrame, pngBytesFromDataUrl,
  type BrowserCapture, type BrowserViewportId,
} from "./browser-point";
import { BROWSER_POINT_CANCEL_SCRIPT, browserPointScript, type BrowserPointMode } from "./browser-point-page";
import {
  browserDownloadNotice, browserZoomFactor, type BrowserEventV1, type BrowserKeyAction,
} from "../shared/browser-events";

/** The `<webview>` methods and events a Browser page uses (Electron's WebviewTag). */
interface WebviewElement extends HTMLElement {
  src: string;
  loadURL(url: string): Promise<void>;
  getURL(): string;
  canGoBack(): boolean;
  canGoForward(): boolean;
  goBack(): void;
  goForward(): void;
  reload(): void;
  stop(): void;
  /** Runs in the page's main world; the result is untrusted page data. */
  executeJavaScript(code: string, userGesture?: boolean): Promise<unknown>;
  /** A rect in the guest's device-independent pixels. Only `toDataURL` works in a sandboxed renderer. */
  capturePage(rect?: { x: number; y: number; width: number; height: number }): Promise<{ toDataURL(): string }>;
  getZoomFactor(): number;
  setZoomFactor(factor: number): void;
  reloadIgnoringCache(): void;
  /** The guest's id in main: how main's Browser events name this page. */
  getWebContentsId(): number;
  findInPage(text: string, options?: { forward?: boolean; findNext?: boolean }): number;
  stopFindInPage(action: "clearSelection" | "keepSelection" | "activateSelection"): void;
}

interface NavigationEvent extends Event { readonly url: string; readonly isMainFrame?: boolean }
interface TitleEvent extends Event { readonly title: string }
interface FoundInPageEvent extends Event {
  readonly result: { readonly requestId: number; readonly activeMatchOrdinal?: number;
    readonly matches?: number; readonly finalUpdate?: boolean };
}
interface FailedLoadEvent extends Event {
  readonly errorCode: number;
  readonly validatedURL: string;
  readonly isMainFrame: boolean;
}

export interface BrowserPageState {
  /** The page shown or being opened: a saved address, else the home page. */
  readonly page: string | null;
  readonly title: string | null;
  readonly loading: boolean;
  readonly back: boolean;
  readonly forward: boolean;
  /** Why the page did not load, in words. */
  readonly failure: string | null;
  /** The host that does not exist, which may have been meant as a search. */
  readonly missingHost: string | null;
  /** Picking an element or marking a region in the page. */
  readonly pointing: BrowserPointMode | null;
  /** The last capture, until it goes to a draft or is dismissed. */
  readonly card: BrowserPointCard | null;
  /** The owner's comment on the capture, and the thread it goes to. */
  readonly comment: string;
  readonly target: string | null;
  /** What became of the last pick or note, in words. */
  readonly pointNotice: string | null;
  /** The width the page is laid out at (Fit fills the pane). */
  readonly viewport: BrowserViewportId;
  /** The find field is open over the page; each ⌘F bumps `findRequest` to focus it again. */
  readonly finding: boolean;
  readonly findRequest: number;
  /** "3 of 12": the match shown and how many there are; null before an answer. */
  readonly found: { readonly active: number; readonly total: number } | null;
  /** Bumped by ⌘L in the page: the address bar takes the keyboard. */
  readonly addressRequest: number;
  /** The last download's line in the pane. */
  readonly download: string | null;
  /** The page is fullscreen, filling the app window. */
  readonly fullscreen: boolean;
}

export interface BrowserPointCard {
  readonly capture: BrowserCapture;
  /** The picture as a PNG `data:` URL for the card, and its bytes for the draft. */
  readonly image: string | null;
  readonly png: Uint8Array | null;
}

export interface BrowserPageEvents {
  /** The main frame arrived somewhere: a load, a redirect or an in-page route. */
  readonly navigated: (url: string) => void;
  readonly titled: (title: string) => void;
  /** Something in the page took keyboard focus. */
  readonly focused: () => void;
  /** A link wants a new tab beside this page. Without it, the page itself goes there. */
  readonly openTab?: (url: string, background: boolean) => void;
}

const EMPTY: BrowserPageState = {
  page: null, title: null, loading: false, back: false, forward: false, failure: null, missingHost: null,
  pointing: null, card: null, comment: "", target: null, pointNotice: null, viewport: "fit",
  finding: false, findRequest: 0, found: null, addressRequest: 0, download: null, fullscreen: false,
};

/** Every page alive, for main's Browser events to find theirs by guest id. */
const LIVE_PAGES = new Set<BrowserPage>();

/**
 * One Browser tab's page. Its `<webview>` sits in a host box that joins the
 * terminal centre's page layer once and never moves: Electron destroys a
 * guest whose element leaves the document, so a pane that split, swapped,
 * tidied or changed workspace would reload its page.
 *
 * The pane shows the page by putting this page's CSS anchor on a slot in its
 * body. The host is anchored over that slot and follows it wherever the
 * layout puts it. A page whose tab is not shown, or whose slot has no box (a
 * maximized neighbour, a flipped card, the launcher), is parked: out of sight
 * at its last size, so the page neither reflows nor loses its scroll, and
 * still running, throttled as a hidden page is.
 */
export class BrowserPage {
  readonly host: HTMLDivElement;
  private view: WebviewElement | null = null;
  private ready = false;
  /** Asked for the keyboard before its first document was ready. */
  private focusWhenReady = false;
  private state: BrowserPageState;
  private readonly listeners = new Set<() => void>();
  private slot: HTMLElement | null = null;
  private readonly slotBox: ResizeObserver;
  /** A saved address, or home, opened the first time the page is shown. */
  private pending: string | null;
  /** Made with no address: its address bar takes the keyboard once, as a new tab's does. */
  private addressFocus: boolean;
  private disposed = false;
  /** The slot's last shown width, for a viewport preset. */
  private slotWidth = 0;
  /** Bumped by every pick, cancel and navigation: a late answer from the page is dropped. */
  private pointToken = 0;
  /** The find request whose answers count; an older one's are dropped. */
  private findId = 0;

  constructor(private readonly anchor: string, private readonly events: BrowserPageEvents,
      url?: string) {
    const saved = url === undefined ? null : browserPageUrl(url);
    this.addressFocus = saved === null;
    this.pending = saved ?? BROWSER_HOME_URL;
    this.state = { ...EMPTY, page: this.pending };
    const host = document.createElement("div");
    host.className = "browser-page";
    host.style.setProperty("position-anchor", anchor);
    host.addEventListener("focusin", () => { this.events.focused(); });
    this.host = host;
    // Before paint: a slot that loses its box parks the page in the same frame.
    this.slotBox = new ResizeObserver(entries => {
      const box = entries.at(-1)?.contentRect;
      if (box === undefined || this.slot === null) return;
      const shown = box.width > 0 && box.height > 0;
      if (shown) {
        host.style.setProperty("--browser-page-width", `${box.width.toString()}px`);
        host.style.setProperty("--browser-page-height", `${box.height.toString()}px`);
        this.slotWidth = box.width;
        this.fitViewport();
      }
      this.park(!shown);
    });
    this.park(true);
    this.cover();
    LIVE_PAGES.add(this);
  }

  /** The live page main's events name, by its guest's webContents id. */
  static forWebContents(id: number): BrowserPage | undefined {
    for (const page of LIVE_PAGES) if (page.webContentsId() === id) return page;
    return undefined;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  readonly snapshot = (): BrowserPageState => this.state;

  /** Show the page over `slot`. A saved address loads now, not while parked. */
  attach(slot: HTMLElement): void {
    if (this.disposed) return;
    if (this.slot !== null && this.slot !== slot) this.detach(this.slot);
    this.slot = slot;
    slot.style.setProperty("anchor-name", this.anchor);
    this.slotBox.observe(slot);
    this.park(false);
    const pending = this.pending;
    this.pending = null;
    if (pending !== null) this.navigate(pending);
  }

  detach(slot: HTMLElement): void {
    if (this.slot !== slot) return;
    this.slotBox.unobserve(slot);
    slot.style.removeProperty("anchor-name");
    this.slot = null;
    this.park(true);
  }

  wantsAddressFocus(): boolean { return this.addressFocus; }

  /** True once, for a page made without an address. */
  claimAddressFocus(): boolean {
    const claim = this.addressFocus;
    this.addressFocus = false;
    return claim;
  }

  navigate(url: string): void {
    if (this.disposed) return;
    this.set({ page: url, failure: null, missingHost: null });
    const existing = this.view;
    if (existing !== null) {
      if (this.ready) void existing.loadURL(url).catch(() => undefined);
      else existing.src = url;
      return;
    }
    const element = document.createElement("webview") as WebviewElement;
    element.className = "browser-page-view";
    // Before src: the session is fixed at the first navigation.
    element.setAttribute("partition", BROWSER_PANE_PARTITION);
    // Lets main see window.open and target=_blank, which it opens here instead.
    element.setAttribute("allowpopups", "");
    element.setAttribute("src", url);
    element.addEventListener("dom-ready", () => {
      this.ready = true;
      this.syncHistory();
      // Only if nothing else took the keyboard while the page was loading.
      const current = document.activeElement;
      if (this.focusWhenReady && !this.host.inert && (current === null || current === document.body)) {
        element.focus();
      }
      this.focusWhenReady = false;
    });
    element.addEventListener("did-start-loading", () => { this.set({ loading: true }); });
    element.addEventListener("did-stop-loading", () => { this.set({ loading: false }); this.syncHistory(); });
    const arrived = (event: Event, loaded: boolean): void => {
      const { url: next, isMainFrame } = event as NavigationEvent;
      if (isMainFrame === false) return;
      // A new document has no picker in it: its layer went with the old one.
      if (loaded) { this.dropPoint(); this.set({ pointNotice: null }); }
      // A new document names itself afresh; an in-page route keeps the title.
      this.set(loaded ? { page: next, failure: null, missingHost: null, title: null } : { page: next });
      this.syncHistory();
      if (loaded) this.events.titled("");
      if (loaded && this.state.finding) this.set({ found: null });
      this.events.navigated(next);
    };
    element.addEventListener("did-navigate", event => { arrived(event, true); });
    element.addEventListener("did-navigate-in-page", event => { arrived(event, false); });
    element.addEventListener("did-fail-load", event => {
      const { errorCode, validatedURL, isMainFrame } = event as FailedLoadEvent;
      if (!isMainFrame) return;
      const failure = browserLoadError(errorCode, validatedURL);
      if (failure === null) return;
      let missingHost: string | null = null;
      if (browserHostNotFound(errorCode)) {
        try { missingHost = new URL(validatedURL).hostname || null; } catch { missingHost = null; }
      }
      this.set({ failure, missingHost });
    });
    element.addEventListener("found-in-page", event => {
      const { result } = event as FoundInPageEvent;
      if (result.requestId !== this.findId || result.matches === undefined) return;
      this.set({ found: { active: result.activeMatchOrdinal ?? 0, total: result.matches } });
    });
    // A page's fullscreen (a video's) fills the app window and gives the
    // pane back exactly as it was when the page leaves it.
    element.addEventListener("enter-html-full-screen", () => { this.setFullscreen(true); });
    element.addEventListener("leave-html-full-screen", () => { this.setFullscreen(false); });
    element.addEventListener("page-title-updated", event => {
      const { title } = event as TitleEvent;
      this.set({ title });
      this.events.titled(title);
    });
    this.host.appendChild(element);
    this.view = element;
    this.fitViewport();
    this.cover();
  }

  back(): void { if (this.ready) this.view?.goBack(); }

  forward(): void { if (this.ready) this.view?.goForward(); }

  /** Stop a load in progress, or load the page again. */
  reloadOrStop(): void {
    const element = this.view;
    if (element === null || !this.ready) return;
    if (this.state.loading) element.stop();
    else { this.set({ failure: null, missingHost: null }); element.reload(); }
  }

  /**
   * The guest's id in main, once it is attached. Not only once it is ready:
   * a tab whose first page is a download (a target=_blank download link)
   * never gets a document, and main's word that the file saved names it.
   */
  webContentsId(): number | null {
    if (this.view === null) return null;
    try { return this.view.getWebContentsId(); } catch { return null; }
  }

  /** What main says about this page. */
  handle(event: BrowserEventV1): void {
    if (this.disposed) return;
    switch (event.kind) {
      case "open-tab":
        if (this.events.openTab !== undefined) this.events.openTab(event.url, event.background);
        else this.navigate(event.url);
        break;
      case "find": this.openFind(); break;
      case "address": this.set({ addressRequest: this.state.addressRequest + 1 }); break;
      case "download": this.set({ download: browserDownloadNotice(event.state, event.name) }); break;
    }
  }

  /** A page chord pressed in the pane (its address bar); main handles them in the page. */
  key(action: BrowserKeyAction): void {
    const view = this.view;
    if (action === "find") { this.openFind(); return; }
    if (action === "address") { this.set({ addressRequest: this.state.addressRequest + 1 }); return; }
    if (view === null || !this.ready) return;
    switch (action) {
      case "zoom-in": case "zoom-out": case "zoom-reset":
        view.setZoomFactor(browserZoomFactor(view.getZoomFactor(),
          action === "zoom-in" ? "in" : action === "zoom-out" ? "out" : "reset"));
        break;
      case "reload": this.set({ failure: null, missingHost: null }); view.reload(); break;
      case "hard-reload": this.set({ failure: null, missingHost: null }); view.reloadIgnoringCache(); break;
      case "back": this.back(); break;
      case "forward": this.forward(); break;
    }
  }

  /** Open the find field, or take the keyboard back to it. */
  openFind(): void {
    this.set({ finding: true, findRequest: this.state.findRequest + 1 });
  }

  /** Find `text` as it is typed (a new search), or the next / previous match. */
  find(text: string, step?: "next" | "previous"): void {
    const view = this.view;
    if (view === null || !this.ready) return;
    if (text === "") {
      this.findId += 1;
      view.stopFindInPage("clearSelection");
      this.set({ found: null });
      return;
    }
    this.findId = view.findInPage(text, { forward: step !== "previous", findNext: step === undefined });
  }

  /** Close the find field; the page keeps the keyboard. */
  closeFind(): void {
    if (!this.state.finding) return;
    this.findId += 1;
    if (this.view !== null && this.ready) this.view.stopFindInPage("clearSelection");
    this.set({ finding: false, found: null });
    this.focus();
  }

  /** Where the page is now; getURL throws until its first dom-ready. */
  currentUrl(): string | null {
    return (this.ready ? this.view?.getURL() : undefined) || this.state.page;
  }

  /** Keyboard to the page; false when there is no page to take it. */
  focus(): boolean {
    if (this.view === null) return false;
    this.view.focus();
    this.focusWhenReady = !this.ready;
    return true;
  }

  /**
   * Pick an element or mark a region in the page. The same mode again stops.
   * Only a page that is ready and on screen can be picked from.
   */
  point(mode: BrowserPointMode): void {
    if (this.state.pointing === mode) { this.cancelPoint(); return; }
    const view = this.view;
    if (view === null || !this.ready || !this.shown()) return;
    this.pointToken += 1;
    const token = this.pointToken;
    // The last notice stays until the next capture: removing its line now
    // would move the page under the pointer.
    this.set({ pointing: mode });
    // Escape reaches the page's picker only while the page has the keyboard.
    view.focus();
    // executeJavaScript throws, rather than rejects, for a guest that is gone.
    Promise.resolve().then(() => view.executeJavaScript(browserPointScript(mode)))
      .then(raw => { void this.pointed(token, raw); }, () => {
        if (token === this.pointToken) this.set({ pointing: null, pointNotice: "The page did not let the picker run." });
      });
  }

  /** Stop picking; the page's layer and listeners go with it. */
  cancelPoint(): void {
    if (this.state.pointing === null) return;
    this.dropPoint();
    const view = this.view;
    if (this.ready && view !== null) {
      void Promise.resolve().then(() => view.executeJavaScript(BROWSER_POINT_CANCEL_SCRIPT)).catch(() => undefined);
    }
  }

  setPointComment(comment: string): void { this.set({ comment }); }

  setPointTarget(target: string | null): void { this.set({ target }); }

  /** Close the card; `notice` says where its note went. */
  closeCard(notice: string | null = null): void {
    this.set({ card: null, comment: "", pointNotice: notice });
  }

  setViewport(viewport: BrowserViewportId): void {
    this.set({ viewport });
    this.fitViewport();
  }

  dispose(): void {
    if (this.disposed) return;
    this.cancelPoint();
    this.disposed = true;
    LIVE_PAGES.delete(this);
    this.slotBox.disconnect();
    this.slot?.style.removeProperty("anchor-name");
    this.slot = null;
    this.host.remove();
    this.view = null;
    this.listeners.clear();
  }

  private set(change: Partial<BrowserPageState>): void {
    const next = { ...this.state, ...change };
    if ((Object.keys(change) as (keyof BrowserPageState)[]).every(key => this.state[key] === next[key])) return;
    this.state = next;
    this.cover();
    for (const listener of [...this.listeners]) listener();
  }

  private setFullscreen(fullscreen: boolean): void {
    this.host.classList.toggle("is-fullscreen", fullscreen);
    this.set({ fullscreen });
  }

  private shown(): boolean {
    return this.slot !== null && !this.host.classList.contains("is-parked");
  }

  private dropPoint(): void {
    this.pointToken += 1;
    this.set({ pointing: null });
  }

  /** The page answered a pick: check it, then take its picture while the page is on screen. */
  private async pointed(token: number, raw: unknown): Promise<void> {
    if (token !== this.pointToken) return;
    const pick = browserPickFromPage(raw);
    if (pick === null) { this.dropPoint(); return; }
    const view = this.view;
    let image: string | null = null;
    let png: Uint8Array | null = null;
    let zoom = 1;
    try { zoom = view?.getZoomFactor() ?? 1; } catch { zoom = 1; }
    const rect = browserCropRect(pick, zoom);
    if (view !== null && rect !== null && this.shown()) {
      try {
        const data = (await view.capturePage(rect)).toDataURL();
        png = pngBytesFromDataUrl(data);
        image = png === null ? null : data;
      } catch { png = null; }
    }
    if (token !== this.pointToken) return;
    this.pointToken += 1;
    this.set({
      pointing: null,
      card: { capture: browserCapture(pick, this.currentUrl(), this.state.title), image, png },
      pointNotice: png === null ? "No picture could be taken of that; the note can still go without one." : null,
    });
  }

  /** A viewport preset lays the page out at its width, centred in the slot. */
  private fitViewport(): void {
    const view = this.view;
    const viewport = this.state.viewport;
    this.host.toggleAttribute("data-viewport", viewport !== "fit");
    if (view === null) return;
    if (viewport === "fit" || this.slotWidth <= 0) {
      view.style.removeProperty("width");
      view.style.removeProperty("margin-left");
      return;
    }
    const frame = browserViewportFrame(this.slotWidth, viewport);
    view.style.width = `${frame.width.toString()}px`;
    view.style.marginLeft = `${frame.left.toString()}px`;
  }

  private syncHistory(): void {
    const element = this.view;
    if (element === null || !this.ready) return;
    this.set({ back: element.canGoBack(), forward: element.canGoForward() });
  }

  /** The pane's own notice shows through while there is no page, or it failed. */
  private cover(): void {
    this.host.classList.toggle("is-covered", this.view === null || this.state.failure !== null);
  }

  private park(parked: boolean): void {
    // A picker never outlives its page being on screen.
    if (parked) this.cancelPoint();
    this.host.classList.toggle("is-parked", parked);
    this.host.inert = parked;
    if (parked) this.host.setAttribute("aria-hidden", "true");
    else this.host.removeAttribute("aria-hidden");
  }
}

/**
 * Every Browser tab's page for one terminal centre. Pages are made the first
 * time their tab is drawn, so a saved workspace loads nothing until it is
 * opened, and live until their tab leaves every workspace's layout.
 */
export class BrowserPages {
  private readonly pages = new Map<string, BrowserPage>();
  private layer: HTMLElement | null = null;
  private anchors = 0;

  constructor(private readonly events: (tabId: string) => BrowserPageEvents) {}

  /** The element the pages live in. Every page joins it once. */
  readonly setLayer = (element: HTMLElement | null): void => {
    this.layer = element;
    if (element === null) return;
    for (const page of this.pages.values()) {
      if (page.host.parentElement !== element) element.appendChild(page.host);
    }
  };

  /** The tab's page, made on first use with its saved address. */
  page(tabId: string, url?: string): BrowserPage {
    let page = this.pages.get(tabId);
    if (page === undefined) {
      this.anchors += 1;
      page = new BrowserPage(`--browser-page-${this.anchors.toString()}`, this.events(tabId), url);
      this.pages.set(tabId, page);
      this.layer?.appendChild(page.host);
    }
    return page;
  }

  get(tabId: string): BrowserPage | undefined {
    return this.pages.get(tabId);
  }

  /** Close every page whose tab is gone. */
  retain(tabIds: ReadonlySet<string>): void {
    for (const [tabId, page] of this.pages) {
      if (tabIds.has(tabId)) continue;
      page.dispose();
      this.pages.delete(tabId);
    }
  }

  /** A tab drag passes over pages to the panes beneath them. */
  setDragging(dragging: boolean): void {
    this.layer?.toggleAttribute("data-tab-drag", dragging);
  }

  dispose(): void {
    this.retain(new Set());
  }
}
