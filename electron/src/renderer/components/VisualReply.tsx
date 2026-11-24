import { useEffect, useRef, useState, type ReactElement } from "react";
import { createPortal } from "react-dom";
import type { SessionKey } from "../../shared/engine.js";
import { VISUAL_FRAME_SANDBOX, type VisualColorScheme } from "../../shared/visual-frame.js";
import type { ConvItem } from "../conversation-reducer";
import {
  appColorScheme, previewFrameUrl, VISUAL_FAILED_TEXT, VISUAL_MISSING_TEXT, visualSizeLabel,
  type LoadedVisual, type VisualFrameApi, type VisualFrameLoader,
} from "../visual-reply";
import { useModalFocus } from "./ModalBehavior";
import { Icon } from "./glyphs";
import "./VisualReply.css";

type VisualItem = Extract<ConvItem, { kind: "visual" }>;

/** The app's colour scheme, following the appearance's change event. */
export function useAppColorScheme(): VisualColorScheme {
  const read = (): VisualColorScheme =>
    appColorScheme(typeof document === "undefined" ? null : document.documentElement);
  const [scheme, setScheme] = useState(read);
  useEffect(() => {
    const root = document.documentElement;
    const changed = (): void => { setScheme(read()); };
    root.addEventListener("interface-theme-change", changed);
    changed();
    return () => { root.removeEventListener("interface-theme-change", changed); };
  }, []);
  return scheme;
}

type Loaded =
  | { state: "loading" }
  | { state: "ready"; visual: LoadedVisual }
  | { state: "missing" }
  | { state: "failed" };

/**
 * The frame a visual page is drawn in: sandboxed to scripts only (no
 * same-origin, forms, popups, top navigation or downloads), served by main
 * from `iface-visual:` under its own CSP. Keyed by URL, so a new page is a
 * new frame: main lets a frame load a visual only when it is empty.
 */
export function VisualFrame({ url, title }: { url: string; title: string }): ReactElement {
  return (
    <iframe key={url} className="visual-reply-frame" src={url} title={title}
      sandbox={VISUAL_FRAME_SANDBOX} referrerPolicy="no-referrer" allow="" />
  );
}

/**
 * A page the agent showed (T3 section 8): its title, its size, "Source" to
 * read the HTML as text, "Open larger" for an overlay over most of the window.
 * No message from the page is ever listened to.
 */
export function VisualReplyCard({ item, sessionKey, loader }: {
  item: VisualItem;
  sessionKey: SessionKey | undefined;
  loader: VisualFrameLoader | undefined;
}): ReactElement {
  const scheme = useAppColorScheme();
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });
  const [source, setSource] = useState(false);
  const [larger, setLarger] = useState(false);
  useEffect(() => {
    if (loader === undefined || sessionKey === undefined) {
      setLoaded({ state: "failed" });
      return;
    }
    let current = true;
    let shown: string | null = null;
    setLoaded({ state: "loading" });
    loader.load(sessionKey, item.visualId, scheme).then(visual => {
      if (!current) {
        if (visual !== null) loader.release(visual.url);
        return;
      }
      if (visual === null) { setLoaded({ state: "missing" }); return; }
      shown = visual.url;
      setLoaded({ state: "ready", visual });
    }, () => { if (current) setLoaded({ state: "failed" }); });
    return () => {
      current = false;
      if (shown !== null) loader.release(shown);
    };
  }, [loader, sessionKey, item.visualId, scheme]);

  const ready = loaded.state === "ready" ? loaded.visual : null;
  return (
    <section className="visual-reply" aria-label={`Visual: ${item.title}`}
      data-visual-id={item.visualId}>
      <VisualHead title={item.title} bytes={item.bytes} ready={ready !== null}
        source={source} onSource={() => { setSource(value => !value); }}
        larger={{ label: "Open larger", onPress: () => { setLarger(true); } }} />
      <VisualBody loaded={loaded} source={source} title={item.title} />
      {larger && ready !== null && (
        <VisualOverlay title={item.title} bytes={item.bytes} visual={ready}
          onClose={() => { setLarger(false); }} />
      )}
    </section>
  );
}

function VisualHead({ title, bytes, ready, source, onSource, larger }: {
  title: string; bytes: number; ready: boolean; source: boolean; onSource: () => void;
  larger: { label: string; onPress: () => void };
}): ReactElement {
  return (
    <div className="visual-reply-head">
      <Icon name="open-preview" className="conv-kind" />
      <span className="visual-reply-title" title={title}>{title}</span>
      <span className="visual-reply-size">{visualSizeLabel(bytes)}</span>
      {ready && (
        <>
          <button type="button" className="visual-reply-button" aria-pressed={source}
            onClick={onSource}>Source</button>
          <button type="button" className="visual-reply-button" onClick={larger.onPress}>
            {larger.label}
          </button>
        </>
      )}
    </div>
  );
}

function VisualBody({ loaded, source, title }: {
  loaded: Loaded; source: boolean; title: string;
}): ReactElement {
  switch (loaded.state) {
    case "loading": return <p className="visual-reply-state" role="status">Loading the visual…</p>;
    case "missing": return <p className="visual-reply-state" role="status">{VISUAL_MISSING_TEXT}</p>;
    case "failed": return <p className="visual-reply-state" role="status">{VISUAL_FAILED_TEXT}</p>;
    case "ready":
      return source
        ? <pre className="visual-reply-source" aria-label={`Source of ${title}`}>{loaded.visual.html}</pre>
        : <VisualFrame url={loaded.visual.url} title={title} />;
  }
}

function VisualOverlay({ title, bytes, visual, onClose }: {
  title: string; bytes: number; visual: LoadedVisual; onClose: () => void;
}): ReactElement {
  const dialogRef = useRef<HTMLElement>(null);
  const [source, setSource] = useState(false);
  const onKey = useModalFocus({ open: true, containerRef: dialogRef, onEscape: onClose });
  const surface = (
    <div className="visual-reply-backdrop" onMouseDown={event => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section ref={dialogRef} className="visual-reply-overlay is-floating" role="dialog" aria-modal="true"
        aria-label={title} tabIndex={-1} onKeyDown={onKey}>
        <VisualHead title={title} bytes={bytes} ready source={source}
          onSource={() => { setSource(value => !value); }}
          larger={{ label: "Close", onPress: onClose }} />
        {source
          ? <pre className="visual-reply-source" aria-label={`Source of ${title}`}>{visual.html}</pre>
          : <VisualFrame url={visual.url} title={title} />}
      </section>
    </div>
  );
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}

/**
 * Output preview's Render (T3 section 8): an `.html` or `.svg` file drawn in
 * the same sandboxed visual frame as a visual reply, sized to the panel.
 */
export function RenderedPagePreview({ text, kind, frames, title }: {
  text: string;
  kind: "html" | "svg";
  frames: VisualFrameApi["visualFrame"];
  title: string;
}): ReactElement {
  const scheme = useAppColorScheme();
  const [url, setUrl] = useState<string | null | "failed">(null);
  useEffect(() => {
    let current = true;
    let shown: string | null = null;
    setUrl(null);
    previewFrameUrl({ visualFrame: frames }, text, kind, scheme).then(registered => {
      if (!current) { void frames({ kind: "release", url: registered }).catch(() => undefined); return; }
      shown = registered;
      setUrl(registered);
    }, () => { if (current) setUrl("failed"); });
    return () => {
      current = false;
      if (shown !== null) void frames({ kind: "release", url: shown }).catch(() => undefined);
    };
  }, [frames, text, kind, scheme]);
  if (url === null) return <p className="visual-reply-state" role="status">Rendering…</p>;
  if (url === "failed") return <p className="visual-reply-state" role="status">This page could not be rendered.</p>;
  return (
    <div className="visual-reply visual-reply-preview">
      <VisualFrame url={url} title={title} />
    </div>
  );
}
