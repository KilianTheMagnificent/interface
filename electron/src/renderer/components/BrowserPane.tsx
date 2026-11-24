import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { browserKeyAction } from "../../shared/browser-events";
import { browserAddress, browserSearchUrl, browserShortAddress } from "../browser-pane";
import { BrowserPage, type BrowserPageState, type BrowserPointCard } from "../browser-page";
import {
  BROWSER_VIEWPORTS, browserCaptureNote, browserCaptureSummary, browserNoteAddedMessage, browserViewportPreset,
  type BrowserNoteImage, type BrowserNoteTarget,
} from "../browser-point";
import { ipcMessage, withoutIpcErrorShape } from "../ipc-message";
import { AppSelect } from "./AppSelect";
import "./BrowserPane.css";

export interface BrowserPaneProps {
  readonly tabId: string;
  /** The tab's page. It outlives this component: a pane that moves keeps it. */
  readonly page: BrowserPage;
  /** Loopback servers the workspace's terminals listen on; undefined until found. */
  readonly servers: readonly string[] | undefined;
  /** This tab is the one its pane shows. */
  readonly active: boolean;
  /** Look (again) for the workspace's dev servers. */
  readonly onFindServers: (tabId: string) => void;
  readonly onOpenExternal?: (url: string) => Promise<void>;
  /** Threads a capture's note can go to: Output preview's feedback targets. */
  readonly pointTargets?: readonly BrowserNoteTarget[];
  /** Puts a capture's note and picture in a thread's draft. Never sends. */
  readonly onAddToDraft?: BrowserAddToDraft;
  /** Copies a capture's note; the clipboard by default. */
  readonly onCopy?: (text: string) => Promise<void>;
}

export type BrowserAddToDraft = (target: string, note: string, png: Uint8Array | null) => Promise<BrowserNoteImage>;

const NO_TARGETS: readonly BrowserNoteTarget[] = [];
const copyToClipboard = (text: string): Promise<void> => navigator.clipboard.writeText(text);
const VIEWPORT_OPTIONS = BROWSER_VIEWPORTS.map(preset => ({
  value: preset.id, label: preset.label, triggerLabel: preset.label.split(" ")[0] ?? preset.label,
}));

const INVALID_ADDRESS = "Only web pages open here. Search, or enter an address such as example.com.";

/**
 * A Browser tab's body in its pane: the address bar, and the slot its page
 * is shown over. A new tab opens Google with the address bar ready; anything
 * typed that is not an address is searched. A dev server running in the
 * workspace is one click away in the bar. Pages run in their own session with
 * no bridge to the app (main pins every `<webview>` before it attaches).
 * Memoized: every terminal title or activity change renders the terminal
 * center, and a Browser has nothing to redraw for it.
 *
 * Drawn like the agent overview (owner, 2026-10-14): a rounded search field
 * with pill chips over the page, which sits in a rounded card.
 */
export const BrowserPane = memo(function BrowserPane({ tabId, page, servers, active, onFindServers,
    onOpenExternal, pointTargets = NO_TARGETS, onAddToDraft, onCopy = copyToClipboard }: BrowserPaneProps) {
  const slot = useRef<HTMLDivElement>(null);
  const address = useRef<HTMLInputElement>(null);
  const state = useSyncExternalStore(page.subscribe, page.snapshot);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(false);
  const [invalid, setInvalid] = useState(false);
  const [external, setExternal] = useState<string | null>(null);
  // Focusing the address shows the full URL, selected, as a browser does; the
  // click that focused it must not then drop a caret into the selection.
  const selectOnMouseUp = useRef(false);
  useLayoutEffect(() => { if (editing) address.current?.select(); }, [editing]);
  const [findQuery, setFindQuery] = useState("");

  // ⌘L in the page: the address bar takes the keyboard, its address selected.
  const addressRequest = state.addressRequest;
  useEffect(() => {
    if (addressRequest === 0 || !active) return;
    address.current?.focus();
    address.current?.select();
  }, [active, addressRequest]);

  // Escape stops a pick from the app's side too, wherever the keyboard is.
  const pointing = state.pointing !== null;
  useEffect(() => {
    if (!pointing) return undefined;
    const stop = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      page.cancelPoint();
    };
    document.addEventListener("keydown", stop, true);
    return () => { document.removeEventListener("keydown", stop, true); };
  }, [page, pointing]);

  // Only the tab its pane shows puts its page on screen. A background tab's
  // slot is skipped by the layout, which no anchor can follow.
  useLayoutEffect(() => {
    const element = slot.current;
    if (element === null || !active) return undefined;
    page.attach(element);
    return () => { page.detach(element); };
  }, [active, page]);

  // The workspace's dev servers, offered in the bar; looked for again
  // whenever the address is focused, since one may have started since.
  useEffect(() => {
    if (servers === undefined) onFindServers(tabId);
  }, [onFindServers, servers, tabId]);
  // A new tab takes the keyboard in its address bar, ready for a search: a
  // frame later, once the menu that opened it and the re-tidied layout settle.
  useEffect(() => {
    if (!active || !page.wantsAddressFocus()) return undefined;
    const frame = requestAnimationFrame(() => {
      if (address.current?.isConnected === true && page.claimAddressFocus()) address.current.focus();
    });
    return () => { cancelAnimationFrame(frame); };
  }, [active, page]);

  const submit = (): void => {
    if (draft.trim() === "") { address.current?.blur(); return; }
    const url = browserAddress(draft);
    if (url === null) { setInvalid(true); return; }
    setEditing(false); setInvalid(false);
    page.navigate(url);
    if (!page.focus()) address.current?.blur();
  };

  const openOutside = (): void => {
    const url = page.currentUrl();
    if (url === null || onOpenExternal === undefined) return;
    setExternal(null);
    onOpenExternal(url).catch((error: unknown) => {
      setExternal(error instanceof Error ? withoutIpcErrorShape(error.message) : "The default browser could not be opened.");
    });
  };

  const shown = editing ? draft : state.page === null ? draft : browserShortAddress(state.page);
  const server = servers?.find(candidate => candidate !== state.page) ?? null;
  const search = state.missingHost === null ? null : browserSearchUrl(state.missingHost);
  return (
    // The page's chords work from the bar as they do in the page (where main
    // takes them): zoom, reload, back and forward act on the page, never the
    // whole app, and ⌘F / ⌘L reach the find field and the address.
    <section className="browser-pane" aria-label="Browser" onKeyDown={event => {
      const action = browserKeyAction({ key: event.key, meta: event.metaKey, control: event.ctrlKey,
        alt: event.altKey, shift: event.shiftKey });
      if (action === null) return;
      event.preventDefault();
      page.key(action);
    }}>
      <form className="browser-pane-bar" onSubmit={event => { event.preventDefault(); submit(); }}>
        <button type="button" className="terminal-center-action" aria-label="Back" title="Back"
          disabled={!state.back} onClick={() => { page.back(); }}>
          <span className="codicon codicon-chevron-left" aria-hidden="true" />
        </button>
        <button type="button" className="terminal-center-action" aria-label="Forward" title="Forward"
          disabled={!state.forward} onClick={() => { page.forward(); }}>
          <span className="codicon codicon-chevron-right" aria-hidden="true" />
        </button>
        <button type="button" className="terminal-center-action"
          aria-label={state.loading ? "Stop" : "Reload"} title={state.loading ? "Stop" : "Reload"}
          disabled={state.page === null} onClick={() => { page.reloadOrStop(); }}>
          <span className={`codicon ${state.loading ? "codicon-close" : "codicon-refresh"}`} aria-hidden="true" />
        </button>
        {/* The whole field takes a click for the address; a click beside the
            text must not leave the next one inside it swallowed. */}
        <div className={`browser-pane-field${invalid ? " is-invalid" : ""}`}
          onMouseDown={event => {
            if (event.target === address.current) return;
            event.preventDefault();
            address.current?.focus();
          }}
          onMouseUp={() => { selectOnMouseUp.current = false; }}>
          <span className={`codicon ${editing || state.page === null ? "codicon-search" : "codicon-globe"}`}
            aria-hidden="true" />
          <input ref={address} className="browser-pane-address"
            aria-label="Address" aria-invalid={invalid || undefined} placeholder="Search Google or type a web address"
            spellCheck={false} autoComplete="off" autoCapitalize="off" maxLength={8192} value={shown}
            onFocus={() => {
              selectOnMouseUp.current = true; setEditing(true); setDraft(state.page ?? draft);
              onFindServers(tabId);
            }}
            onMouseUp={event => { if (selectOnMouseUp.current) event.preventDefault(); selectOnMouseUp.current = false; }}
            onBlur={() => { selectOnMouseUp.current = false; setEditing(false); setInvalid(false); }}
            onChange={event => { setDraft(event.currentTarget.value); setInvalid(false); }}
            onKeyDown={event => {
              if (event.key !== "Escape") return;
              event.preventDefault();
              setDraft(state.page ?? ""); setInvalid(false); event.currentTarget.blur();
            }} />
          {state.loading && <span className="browser-pane-progress" role="progressbar" aria-label="Loading" />}
        </div>
        {server !== null && <button type="button" className="browser-pane-server"
          aria-label={`Open ${browserShortAddress(server)}, running in this workspace`}
          title={`Open ${browserShortAddress(server)}, running in this workspace`}
          onClick={() => { page.navigate(server); }}>
          <span className="browser-pane-server-name">{browserShortAddress(server)}</span>
        </button>}
        <button type="button" className="browser-pane-choice browser-pane-point" aria-pressed={state.pointing === "element"}
          title="Pick an element on the page to comment on" disabled={state.page === null}
          onClick={() => { page.point("element"); }}>Pick</button>
        <button type="button" className="browser-pane-choice browser-pane-point" aria-pressed={state.pointing === "region"}
          title="Mark a region of the page to comment on" disabled={state.page === null}
          onClick={() => { page.point("region"); }}>Region</button>
        <AppSelect compact className="browser-pane-viewport" label="Page width" value={state.viewport}
          options={VIEWPORT_OPTIONS} onChange={value => { page.setViewport(browserViewportPreset(value).id); }} />
        {onOpenExternal !== undefined && <button type="button" className="terminal-center-action"
          aria-label="Open in default browser" title="Open in default browser"
          disabled={state.page === null} onClick={openOutside}>
          <span className="codicon codicon-link-external" aria-hidden="true" />
        </button>}
      </form>
      {state.card !== null && <BrowserPointCardView page={page} state={state} card={state.card}
        targets={pointTargets} onAddToDraft={onAddToDraft} onCopy={onCopy} />}
      {state.card === null && state.pointNotice !== null
        && <p className="browser-pane-hint" role="status">{state.pointNotice}</p>}
      {state.download !== null && <p className="browser-pane-hint browser-pane-download" role="status">{state.download}</p>}
      {state.finding && active && createPortal(<BrowserFindField page={page} state={state}
        query={findQuery} onQuery={setFindQuery} />, page.host)}
      <div className="browser-pane-body">
        {/* The page is drawn over this box, from its page layer. */}
        <div ref={slot} className="browser-pane-slot" />
        {state.failure !== null && <div className="browser-pane-notice">
          <span className="codicon codicon-globe" aria-hidden="true" />
          <p role="alert">{state.failure}</p>
          <div className="browser-pane-choices">
            {search !== null && <button type="button" className="browser-pane-choice"
              onClick={() => { page.navigate(search); }}>Search Google for {state.missingHost}</button>}
            <button type="button" className="browser-pane-choice"
              onClick={() => { if (state.page !== null) page.navigate(state.page); }}>Retry</button>
          </div>
        </div>}
      </div>
      {invalid && <p className="browser-pane-status" role="alert">{INVALID_ADDRESS}</p>}
      {external !== null && <p className="browser-pane-status" role="alert">{external}</p>}
    </section>
  );
});

interface BrowserFindFieldProps {
  readonly page: BrowserPage;
  readonly state: BrowserPageState;
  readonly query: string;
  readonly onQuery: (query: string) => void;
}

/**
 * Find on page, over the page's top right: the terminal pane's search field,
 * drawn the same. Matches as you type and says "3 of 12"; Enter / ⇧Enter and
 * the arrows step; Esc closes it and gives the page the keyboard.
 */
function BrowserFindField({ page, state, query, onQuery }: BrowserFindFieldProps) {
  const input = useRef<HTMLInputElement>(null);
  const request = state.findRequest;
  useLayoutEffect(() => { input.current?.focus(); input.current?.select(); }, [request]);
  const step = (direction: "next" | "previous"): void => { if (query !== "") page.find(query, direction); };
  const found = state.found;
  const status = query === "" || found === null ? "" : found.total === 0 ? "No match"
    : `${found.active.toString()} of ${found.total.toString()}`;
  return (
    <div className="terminal-search is-floating browser-find" role="search">
      <input ref={input} value={query} placeholder="Find" aria-label="Find in page"
        spellCheck={false} autoComplete="off" maxLength={1000}
        onChange={event => { const next = event.currentTarget.value; onQuery(next); page.find(next); }}
        onKeyDown={event => {
          if (event.key === "Escape") { event.preventDefault(); page.closeFind(); }
          else if (event.key === "Enter") { event.preventDefault(); step(event.shiftKey ? "previous" : "next"); }
          else if (event.key === "ArrowDown") { event.preventDefault(); step("next"); }
          else if (event.key === "ArrowUp") { event.preventDefault(); step("previous"); }
        }} />
      <span className="terminal-search-result" role="status">{status}</span>
      <button type="button" aria-label="Previous match" title="Previous match"
        disabled={query === ""} onClick={() => { step("previous"); }}>
        <span className="codicon codicon-chevron-up" aria-hidden="true" />
      </button>
      <button type="button" aria-label="Next match" title="Next match"
        disabled={query === ""} onClick={() => { step("next"); }}>
        <span className="codicon codicon-chevron-down" aria-hidden="true" />
      </button>
      <button type="button" aria-label="Close find" title="Close"
        onClick={() => { page.closeFind(); }}>
        <span className="codicon codicon-close" aria-hidden="true" />
      </button>
    </div>
  );
}

interface BrowserPointCardProps {
  readonly page: BrowserPage;
  readonly state: BrowserPageState;
  readonly card: BrowserPointCard;
  readonly targets: readonly BrowserNoteTarget[];
  readonly onAddToDraft: BrowserAddToDraft | undefined;
  readonly onCopy: (text: string) => Promise<void>;
}

/**
 * What a pick or a region captured, shown before it goes anywhere: its
 * picture, what it is in plain words, the selector, the owner's comment, and
 * the thread whose draft gets the note. Page text is drawn as text only.
 */
function BrowserPointCardView({ page, state, card, targets, onAddToDraft, onCopy }: BrowserPointCardProps) {
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pick = card.capture.pick;
  const target = targets.find(candidate => candidate.key === state.target) ?? null;
  const note = (): string => browserCaptureNote(card.capture, state.comment);
  const add = (): void => {
    if (target === null || onAddToDraft === undefined || adding) return;
    setAdding(true); setError(null);
    onAddToDraft(target.key, note(), card.png).then(image => {
      setAdding(false);
      page.closeCard(browserNoteAddedMessage(target.title, image));
    }, (failure: unknown) => {
      setAdding(false);
      setError(ipcMessage(failure));
    });
  };
  const copy = (): void => {
    setError(null);
    onCopy(note()).then(() => { page.closeCard("Note copied. Paste it where the agent will read it."); },
      (failure: unknown) => { setError(ipcMessage(failure)); });
  };
  return (
    <div className="browser-point-card" role="group" aria-label="Capture from the page">
      {card.image !== null && <img className="browser-point-thumb" src={card.image}
        alt={pick.kind === "region" ? "The marked region" : "The picked element"} />}
      <div className="browser-point-main">
        <p className="browser-point-summary">{browserCaptureSummary(card.capture)}</p>
        {pick.kind === "element" && pick.selector !== null
          && <code className="browser-point-selector" title={pick.selector}>{pick.selector}</code>}
        {pick.kind === "element" && pick.frame && <div className="browser-pane-choices">
          <button type="button" className="browser-pane-choice" onClick={() => { page.point("region"); }}>Mark a region instead</button>
        </div>}
        <textarea className="browser-point-comment" aria-label="Comment on the capture" maxLength={4000}
          placeholder="What should the agent change here?" value={state.comment}
          onChange={event => { page.setPointComment(event.currentTarget.value); }} />
        <div className="browser-point-foot">
          {onAddToDraft !== undefined && <AppSelect compact className="browser-point-target" label="Thread for the note"
            placeholder={targets.length === 0 ? "No threads open" : "Choose a thread"} value={target?.key ?? ""}
            disabled={targets.length === 0 || adding}
            options={targets.map(candidate => ({ value: candidate.key, label: candidate.title }))}
            onChange={value => { page.setPointTarget(value === "" ? null : value); }} />}
          <button type="button" className="terminal-center-action" aria-label="Copy note" title="Copy note"
            onClick={copy}><span className="codicon codicon-copy" aria-hidden="true" /></button>
          <span className="browser-point-spacer" />
          <button type="button" className="btn" onClick={() => { page.closeCard(); }}>Dismiss</button>
          {onAddToDraft !== undefined && <button type="button" className="btn btn-primary browser-point-add"
            disabled={target === null || adding} onClick={add}>{adding ? "Adding…" : "Add to draft"}</button>}
        </div>
        {error !== null && <p className="browser-point-error" role="alert">{error}</p>}
      </div>
    </div>
  );
}

/** Not a layout tab id: the column's page is no pane's. */
export const BROWSER_COLUMN_ID = "browser-column";
const BROWSER_COLUMN_ANCHOR = "--browser-page-column";

export interface BrowserColumnProps {
  /** The page it last showed this session; absent, it opens home with the address ready. */
  readonly url: string | undefined;
  /** The Browser tab is the one the column shows. */
  readonly active: boolean;
  readonly servers: readonly string[] | undefined;
  readonly onFindServers: (tabId: string) => void;
  readonly onNavigated: (url: string) => void;
  readonly onOpenExternal?: (url: string) => Promise<void>;
  readonly pointTargets?: readonly BrowserNoteTarget[];
  readonly onAddToDraft?: BrowserAddToDraft;
}

/**
 * The agent overview's Browser tab (owner, 2026-10-14): one Browser beside
 * the panes, whichever workspace shows. The column never moves, so its page
 * lives in a layer of its own here rather than the centre's. Closing the
 * column closes the page; opening it again reloads the address it was on.
 */
export function BrowserColumn({ url, active, servers, onFindServers, onNavigated,
    onOpenExternal, pointTargets, onAddToDraft }: BrowserColumnProps) {
  const navigated = useRef(onNavigated);
  navigated.current = onNavigated;
  const [page] = useState(() => new BrowserPage(BROWSER_COLUMN_ANCHOR, {
    navigated: next => { navigated.current(next); }, titled: () => undefined, focused: () => undefined,
  }, url));
  useEffect(() => () => { page.dispose(); }, [page]);
  const layer = useCallback((element: HTMLDivElement | null): void => {
    if (element !== null && page.host.parentElement !== element) element.appendChild(page.host);
  }, [page]);
  return <>
    <BrowserPane tabId={BROWSER_COLUMN_ID} page={page} servers={servers} active={active}
      onFindServers={onFindServers} onOpenExternal={onOpenExternal}
      pointTargets={pointTargets} onAddToDraft={onAddToDraft} />
    <div ref={layer} className="browser-page-layer" />
  </>;
}
