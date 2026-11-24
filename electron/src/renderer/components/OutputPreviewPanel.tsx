import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useRef, useState, type AnimationEvent, type PointerEvent } from "react";
import { presenceClass, type PresencePhase } from "../presence";
import type { IfaceApi } from "../../shared/ipc";
import type { OutputPreviewEntry, OutputPreviewSnapshot, OutputReview } from "../../shared/output-preview";
import { Markdown } from "../markdown";
import { lastSegment } from "../display-paths";
import { AppSelect } from "./AppSelect";
import { ipcMessage } from "../ipc-message";
import { inlineImageSnapshot, inlineTextSnapshot, outputAnchorLabel, outputFeedback,
  outputImageUrl, outputKey, outputPath, outputRegion, outputTextAnchor,
  type OutputAnchor } from "../output-preview";
import { renderableKind } from "../visual-reply";
import { RenderedPagePreview } from "./VisualReply";
import "./OutputPreviewPanel.css";

const PdfPreview = lazy(() => import("./OutputPdfPreview"));
const MAX_OPEN_OUTPUTS = 8;
const IMAGE_ZOOM_STEPS = [0.5, 0.75, 1, 1.5, 2, 3, 4] as const;

export interface OutputPreviewOpenRequest {
  path: string;
  nonce: number;
  root?: string;
  directory?: boolean;
  inlineImage?: string;
  inlineText?: { text: string; format?: "plain" | "markdown" | "diff" };
}

interface OpenOutput {
  panelRoot?: string;
  snapshot: OutputPreviewSnapshot;
  anchor: OutputAnchor;
  note: string;
  latest?: OutputPreviewSnapshot;
  checked?: boolean;
  saved?: OutputReview;
  threadKey?: string | null;
  task?: string;
}
function outputScopeRoot(item: OpenOutput): string {
  return item.panelRoot ?? item.snapshot.root;
}
function dirtyDraft(item: OpenOutput): boolean {
  return item.saved !== undefined && (item.note !== item.saved.note
    || JSON.stringify(item.anchor) !== JSON.stringify(item.saved.anchor)
    || (item.threadKey ?? null) !== item.saved.threadKey || (item.task ?? "") !== item.saved.task);
}
export interface OutputPreviewPanelProps {
  root: string | null;
  visible: boolean;
  /** `visualFrame` (T3 section 8) lets an `.html` or `.svg` file render in a
   * sandboxed frame; without it the file shows as source only. */
  api: Pick<IfaceApi, "previewOutput"> & Partial<Pick<IfaceApi, "visualFrame">>;
  onClose: () => void;
  onOpenFolder: (root: string) => void;
  onCopy?: (text: string) => Promise<void>;
  openRequest?: OutputPreviewOpenRequest | null;
  initialThreadKey?: string;
  feedbackTargets?: readonly { key: string; title: string }[];
  onSendFeedback?: (key: string, text: string) => Promise<"sent" | "unknown" | "unsent">;
  /** Sliding in after the toggle, or out (inert) after a close. */
  phase?: PresencePhase;
  onAnimationEnd?: (event: AnimationEvent<Element>) => void;
}

/**
 * A file's text, a diff drawn line by line. Memoized on the text: a diff can
 * run to 10,000 lines, and typing feedback below it must not rebuild them.
 */
const OutputSource = memo(function OutputSource({ text, diff }: { text: string; diff: boolean }) {
  return <pre className={diff ? "output-source is-diff" : "output-source"}>
    {diff ? text.split("\n").map((line, i) =>
      <span key={i} className={line.startsWith("+") ? "is-added" : line.startsWith("-") ? "is-removed" : line.startsWith("@@") ? "is-hunk" : undefined}>{line}{"\n"}</span>) : text}
  </pre>;
});

function ImagePreview({ snapshot, anchor, onAnchor }: {
  snapshot: OutputPreviewSnapshot; anchor: OutputAnchor; onAnchor: (anchor: OutputAnchor) => void;
}) {
  const [drag, setDrag] = useState<OutputAnchor | null>(null);
  const [failed, setFailed] = useState(false);
  const [zoom, setZoom] = useState<number | null>(null);
  const [lightbox, setLightbox] = useState(false);
  const [naturalWidth, setNaturalWidth] = useState(0);
  const start = useRef<{ x: number; y: number } | null>(null);
  const image = useRef<HTMLImageElement>(null);
  const url = useMemo(() => snapshot.content.kind === "image"
    ? outputImageUrl(snapshot.content.mediaType, snapshot.content.bytes) : "", [snapshot]);
  useEffect(() => {
    if (!lightbox) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.stopPropagation(); setLightbox(false); }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => { window.removeEventListener("keydown", onKeyDown, true); };
  }, [lightbox]);
  const point = (event: PointerEvent<HTMLDivElement>) => {
    const box = image.current?.getBoundingClientRect();
    return !box ? { x: 0, y: 0 } : { x: (event.clientX - box.left) / box.width, y: (event.clientY - box.top) / box.height };
  };
  const stepZoom = (direction: -1 | 1) => {
    setZoom(current => {
      const active = current ?? 1;
      const index = IMAGE_ZOOM_STEPS.findIndex(step => step >= active);
      const from = index < 0 ? IMAGE_ZOOM_STEPS.length - 1 : index;
      const next = Math.max(0, Math.min(IMAGE_ZOOM_STEPS.length - 1, from + direction));
      return IMAGE_ZOOM_STEPS[next];
    });
  };
  const region = drag ?? anchor;
  if (failed) return <p className="output-message" role="alert">This image could not be decoded. Open it in its own application.</p>;
  const zoomedWidth = zoom === null ? undefined : `${Math.max(48, Math.round((naturalWidth || 360) * zoom)).toString()}px`;
  return <div className="output-image-scroll">
    <div className="output-image-toolbar">
      <p className="output-hint">Drag on the image to mark a region for feedback.</p>
      <div className="output-image-controls" role="group" aria-label="Image zoom controls">
        <button type="button" aria-label="Zoom out image"
          disabled={zoom !== null && zoom <= IMAGE_ZOOM_STEPS[0]}
          onClick={() => { stepZoom(-1); }}>−</button>
        <button type="button" aria-label="Reset image zoom"
          onClick={() => { setZoom(current => current === null ? 1 : null); }}>
          {zoom === null ? "Fit" : `${Math.round(zoom * 100).toString()}%`}
        </button>
        <button type="button" aria-label="Zoom in image"
          disabled={zoom !== null && zoom >= IMAGE_ZOOM_STEPS[IMAGE_ZOOM_STEPS.length - 1]}
          onClick={() => { stepZoom(1); }}>+</button>
        <button type="button" aria-label="Expand image in lightbox"
          onClick={() => { setLightbox(true); }}>Expand</button>
      </div>
    </div>
    <div className="output-image" style={zoomedWidth === undefined ? undefined : { maxWidth: "none" }} onPointerDown={event => {
      if (event.button !== 0) return;
      event.preventDefault(); start.current = point(event); event.currentTarget.setPointerCapture(event.pointerId);
      setDrag(outputRegion(start.current, start.current));
    }} onPointerMove={event => { if (start.current) setDrag(outputRegion(start.current, point(event))); }}
    onPointerUp={event => {
      if (!start.current) return;
      const next = outputRegion(start.current, point(event)); start.current = null; setDrag(null);
      if (next.kind === "region" && next.width > 0.005 && next.height > 0.005) onAnchor(next);
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    }} onPointerCancel={() => { start.current = null; setDrag(null); }}
    onDoubleClick={() => { setLightbox(true); }}>
      <img ref={image} src={url} alt={snapshot.path} draggable={false}
        style={zoomedWidth === undefined ? undefined : { width: zoomedWidth, maxWidth: "none" }}
        onLoad={event => { setNaturalWidth(event.currentTarget.naturalWidth); }}
        onError={() => { setFailed(true); }} />
      {region.kind === "region" && <span className="output-image-region" aria-label={outputAnchorLabel(region)}
        style={{ left: `${(region.x * 100).toString()}%`, top: `${(region.y * 100).toString()}%`,
          width: `${(region.width * 100).toString()}%`, height: `${(region.height * 100).toString()}%` }} />}
    </div>
    {lightbox && <div className="output-image-lightbox" role="dialog" aria-modal="true"
      aria-label={`Expanded preview of ${snapshot.path}`}
      onClick={event => { if (event.target === event.currentTarget) setLightbox(false); }}>
      <div className="output-image-lightbox-head">
        <strong>{snapshot.path}</strong>
        <div className="output-image-controls" role="group" aria-label="Lightbox zoom controls">
          <button type="button" aria-label="Zoom out lightbox image"
            disabled={zoom !== null && zoom <= IMAGE_ZOOM_STEPS[0]}
            onClick={() => { stepZoom(-1); }}>−</button>
          <button type="button" aria-label="Reset lightbox image zoom"
            onClick={() => { setZoom(current => current === null ? 1 : null); }}>
            {zoom === null ? "Fit" : `${Math.round(zoom * 100).toString()}%`}
          </button>
          <button type="button" aria-label="Zoom in lightbox image"
            disabled={zoom !== null && zoom >= IMAGE_ZOOM_STEPS[IMAGE_ZOOM_STEPS.length - 1]}
            onClick={() => { stepZoom(1); }}>+</button>
          <button type="button" aria-label="Close expanded image"
            onClick={() => { setLightbox(false); }}>×</button>
        </div>
      </div>
      <div className="output-image-lightbox-body"
        onClick={event => { if (event.target === event.currentTarget) setLightbox(false); }}>
        <img src={url} alt={`Expanded ${snapshot.path}`} draggable={false}
          style={zoomedWidth === undefined ? undefined : { width: zoomedWidth, maxWidth: "none", maxHeight: "none" }} />
      </div>
    </div>}
  </div>;
}

const copyToClipboard = (text: string): Promise<void> => navigator.clipboard.writeText(text);

/** UI-only open tabs and review drafts. Rust owns every read and revision.
 *  Memoized: the terminal center redraws on every terminal's activity. */
export const OutputPreviewPanel = memo(function OutputPreviewPanel({ root, visible, api, onClose,
    onOpenFolder, onCopy = copyToClipboard, openRequest = null, initialThreadKey, feedbackTargets = [], onSendFeedback, phase = "still", onAnimationEnd }: OutputPreviewPanelProps) {
  const [reviews, setReviews] = useState<readonly OutputReview[]>([]);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [sending, setSending] = useState(false);
  const [transitioning, setTransitioning] = useState(false);
  const transitionRef = useRef(false);
  const [compare, setCompare] = useState(false);
  const [outputs, setOutputs] = useState<readonly OpenOutput[]>([]);
  const [selected, setSelected] = useState<Readonly<Record<string, string>>>({});
  const [directory, setDirectory] = useState("");
  const [entries, setEntries] = useState<readonly OutputPreviewEntry[]>([]);
  const [path, setPath] = useState("");
  const [filter, setFilter] = useState("");
  const [browse, setBrowse] = useState(true);
  const [loading, setLoading] = useState(false);
  const [checking, setChecking] = useState(false);
  const [listing, setListing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [plain, setPlain] = useState(false);
  // The file shown rendered (T3 section 8); every file opens as source.
  const [renderedKey, setRenderedKey] = useState<string | null>(null);
  const [closeDraft, setCloseDraft] = useState<string | null>(null);
  const body = useRef<HTMLDivElement>(null);
  const listRequest = useRef(0);
  const fileRequest = useRef(0);
  const checkRequest = useRef(0);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const tabs = useRef<HTMLDivElement>(null);
  const currentRoot = useRef(root);
  currentRoot.current = root;
  const openRequestRef = useRef(openRequest);
  openRequestRef.current = openRequest;
  const key = root ? selected[root] : undefined;
  const current = outputs.find(item => outputKey(item.snapshot.root, item.snapshot.path) === key);
  const snapshot = current?.snapshot;
  // An `.html` or `.svg` text file renders only on request, in the visual
  // frame; Rust still treats it as inert text.
  const renderable = snapshot?.content.kind === "text" && api.visualFrame !== undefined
    ? renderableKind(snapshot.path) : null;
  const rendered = renderable !== null && key !== undefined && renderedKey === key;
  const projectOutputs = outputs.filter(item => outputScopeRoot(item) === root);
  const update = useCallback((target: string, fn: (value: OpenOutput) => OpenOutput) => {
    setOutputs(items => items.map(item => outputKey(item.snapshot.root, item.snapshot.path) === target ? fn(item) : item));
  }, []);
  const pendingDraft = outputs.find(item => item.saved?.delivery !== "unknown" && item.saved?.delivery !== "sending" && dirtyDraft(item));
  const acceptReview = useCallback((review: OutputReview) => {
    setReviews(items => [...items.filter(item => item.id !== review.id), review]);
    update(outputKey(review.snapshot.root, review.snapshot.path), item => item.snapshot.revision === review.snapshot.revision ? ({ ...item, saved: review }) : item);
  }, [update]);
  const saveDraft = useCallback(async (item: OpenOutput): Promise<OutputReview | null> => {
    if (!item.saved || savingRef.current) return null;
    if (!dirtyDraft(item)) return item.saved;
    savingRef.current = true; setSaving(true);
    try {
      const response = await api.previewOutput({ version: 2, action: "saveReview", root: item.snapshot.root,
        id: item.saved.id, revision: item.saved.revision, anchor: item.anchor, note: item.note,
        threadKey: item.threadKey ?? null, task: item.task ?? "" });
      if (response.kind !== "review") throw new Error("The output review was not saved.");
      acceptReview(response.review); return response.review;
    } catch (failure) { setError(ipcMessage(failure)); return null; }
    finally { savingRef.current = false; setSaving(false); }
  }, [acceptReview, api]);
  useEffect(() => {
    if (!pendingDraft || error || saving || transitioning) return;
    const timer = window.setTimeout(() => { void saveDraft(pendingDraft); }, 300);
    return () => { window.clearTimeout(timer); };
  }, [pendingDraft, error, saveDraft, saving, transitioning]);
  useEffect(() => {
    if (!pendingDraft && !saving && !transitioning) return;
    const preventDraftLoss = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ""; };
    window.addEventListener("beforeunload", preventDraftLoss);
    return () => { window.removeEventListener("beforeunload", preventDraftLoss); };
  }, [pendingDraft, saving, transitioning]);
  const transition = async (action: () => Promise<void>) => {
    if (transitionRef.current) return;
    if (savingRef.current || sending) { setNotice("Wait for the current draft save or delivery before changing this review."); return; }
    transitionRef.current = true; setTransitioning(true);
    try { await action(); }
    catch (failure) { setError(ipcMessage(failure)); }
    finally { transitionRef.current = false; setTransitioning(false); }
  };
  const closePanel = () => { void transition(async () => {
    // Flush every open tab before the parent can unmount this panel. A failed
    // save leaves the drafts and panel in place for a deliberate retry.
    for (const item of outputs) if (dirtyDraft(item) && !(await saveDraft(item))) return;
    onClose();
  }); };
  const changeAnchor = useCallback((anchor: OutputAnchor) => {
    if (key && !transitionRef.current) { update(key, item => item.saved?.delivery === "sending" || item.saved?.delivery === "unknown" ? item : ({ ...item, anchor })); setNotice(null); }
  }, [key, update]);
  const annotatePage = useCallback((page: number) => { changeAnchor({ kind: "page", page }); }, [changeAnchor]);
  const list = useCallback(async (folder: string, silent = false) => {
    if (!root) return;
    const request = ++listRequest.current;
    setListing(true); if (!silent) setError(null);
    try {
      const response = await api.previewOutput({ version: 1, action: "list", root, path: folder });
      if (request !== listRequest.current || currentRoot.current !== root) return;
      if (response.kind !== "directory") throw new Error("The project folder could not be listed.");
      setEntries(response.entries); setDirectory(response.path); setFilter("");
    } catch (failure) { if (!silent && request === listRequest.current && currentRoot.current === root) setError(ipcMessage(failure)); }
    finally { if (request === listRequest.current) setListing(false); }
  }, [api, root]);
  useEffect(() => {
    ++fileRequest.current; ++checkRequest.current;
    const hasPendingOpen = openRequestRef.current !== null && opened.current !== openRequestRef.current.nonce;
    setLoading(false); setChecking(false); setEntries([]); setDirectory(""); setPath("");
    setError(null); setNotice(null); setBrowse(root ? !selectedRef.current[root] && !hasPendingOpen : true); setCloseDraft(null);
    if (root) {
      void list("", hasPendingOpen);
      void api.previewOutput({ version: 2, action: "listReviews", root }).then(response => {
        if (currentRoot.current !== root || response.kind !== "reviews") return;
        setReviews(items => [...items.filter(item => item.snapshot.root !== root), ...response.reviews]);
      }).catch(failure => { if (!hasPendingOpen && currentRoot.current === root) setError(ipcMessage(failure)); });
    }
    return () => { ++listRequest.current; ++fileRequest.current; ++checkRequest.current; };
  }, [root, list, api]);
  useEffect(() => { ++checkRequest.current; setChecking(false); setPlain(false); setCompare(false); setNotice(null); setCloseDraft(null); }, [key]);

  useEffect(() => {
    const container = tabs.current;
    const active = container?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!container || !active) return;
    const outer = container.getBoundingClientRect(), inner = active.getBoundingClientRect();
    if (inner.right > outer.right) container.scrollLeft += inner.right - outer.right + 24;
    else if (inner.left < outer.left) container.scrollLeft -= outer.left - inner.left;
  }, [key, root, outputs.length]);

  const open = async (relativePath: string, options?: OutputPreviewOpenRequest) => {
    if (!root || !relativePath.trim()) return;
    const name = relativePath.trim();
    const targetRoot = options?.root?.trim() || root;
    const target = outputKey(targetRoot, name);
    const existing = outputs.find(item => outputKey(item.snapshot.root, item.snapshot.path) === target);
    if (existing) { setSelected(values => ({ ...values, [root]: target })); setBrowse(false); setPath(""); return; }
    if (outputs.length >= MAX_OPEN_OUTPUTS) { setError("Close a preview before opening another. Up to eight files can stay open across projects."); return; }
    if (options?.inlineText) {
      const inlineSnapshot = inlineTextSnapshot(targetRoot, name, options.inlineText.text, options.inlineText.format ?? "plain");
      setOutputs(items => [...items, { snapshot: inlineSnapshot, panelRoot: root, anchor: { kind: "file" }, note: "" }]);
      setSelected(values => ({ ...values, [root]: target })); setBrowse(false); setPath("");
      return;
    }
    const request = ++fileRequest.current;
    setLoading(true); setError(null); setNotice(null);
    try {
      let response: Awaited<ReturnType<typeof api.previewOutput>>;
      try {
        response = await api.previewOutput({ version: 1, action: "read", root: targetRoot, path: name });
      } catch (readFailure) {
        const inlineSnapshot = options?.inlineImage ? inlineImageSnapshot(targetRoot, name, options.inlineImage) : null;
        if (!inlineSnapshot) throw readFailure;
        if (request !== fileRequest.current || currentRoot.current !== root) return;
        setOutputs(items => [...items, { snapshot: inlineSnapshot, panelRoot: root, anchor: { kind: "file" }, note: "" }]);
        setSelected(values => ({ ...values, [root]: target })); setBrowse(false); setPath("");
        return;
      }
      if (request !== fileRequest.current || currentRoot.current !== root) return;
      if (response.kind !== "file") throw new Error("The output could not be opened.");
      let saved: OutputReview | undefined;
      if (targetRoot === root) {
        try {
          // Read saved drafts for this open, not the asynchronously loaded sidebar
          // mirror: a requested file can arrive before the initial list finishes.
          const retained = await api.previewOutput({ version: 2, action: "listReviews", root });
          if (request !== fileRequest.current || currentRoot.current !== root) return;
          if (retained.kind !== "reviews") throw new Error("Saved output reviews could not be read.");
          const remembered = retained.reviews.filter(item => item.snapshot.root === root && item.snapshot.path === name)
            .sort((a, b) => b.updatedAt - a.updatedAt)[0];
          saved = remembered;
          if (!saved && response.snapshot.revision) {
            const captured = await api.previewOutput({ version: 2, action: "captureReview", root, path: name });
            if (request !== fileRequest.current || currentRoot.current !== root) return;
            if (captured.kind !== "review") throw new Error("Could not save this output version for review.");
            saved = captured.review; acceptReview(saved);
          }
        } catch (reviewFailure) {
          if (!options) throw reviewFailure;
        }
      }
      setOutputs(items => [...items, saved ? { snapshot: saved.snapshot, panelRoot: root, anchor: saved.anchor, note: saved.note,
        saved, threadKey: saved.threadKey ?? initialThreadKey, task: saved.task } : { snapshot: response.snapshot, panelRoot: root, anchor: { kind: "file" }, note: "" }]);
      setSelected(values => ({ ...values, [root]: target })); setBrowse(false); setPath("");
    } catch (failure) { if (request === fileRequest.current && currentRoot.current === root) setError(ipcMessage(failure)); }
    finally { if (request === fileRequest.current) setLoading(false); }
  };
  const check = async () => {
    if (!snapshot || !key) return;
    const request = ++checkRequest.current;
    setChecking(true); setNotice(null); setError(null);
    try {
      const response = await api.previewOutput({ version: 1, action: "read", root: snapshot.root, path: snapshot.path });
      if (request !== checkRequest.current || currentRoot.current !== root) return;
      if (response.kind !== "file") throw new Error("The file could not be checked.");
      if (snapshot.revision === null) {
        update(key, item => ({ ...item, snapshot: response.snapshot, latest: undefined,
          anchor: { kind: "file" }, note: "", checked: true }));
        setNotice("Preview refreshed.");
      } else if (response.snapshot.revision !== snapshot.revision) {
        update(key, item => ({ ...item, latest: response.snapshot, checked: true }));
      } else { update(key, item => ({ ...item, latest: undefined, checked: true })); setNotice("This is the current file version."); }
    } catch (failure) { if (request === checkRequest.current) setError(ipcMessage(failure)); }
    finally { if (request === checkRequest.current) setChecking(false); }
  };
  const loadLatest = async () => {
    if (!current || !key) return;
    const saved = await saveDraft(current);
    if (!saved) return;
    try {
      const response = await api.previewOutput({ version: 2, action: "captureReview", root: current.snapshot.root, path: current.snapshot.path });
      if (response.kind !== "review") throw new Error("The latest output could not be saved.");
      acceptReview(response.review);
      update(key, item => ({ snapshot: response.review.snapshot, panelRoot: item.panelRoot, anchor: response.review.anchor,
        note: response.review.note, saved: response.review, threadKey: response.review.threadKey,
        task: response.review.task, checked: true }));
      setCompare(false); setNotice("Earlier feedback is retained in saved versions.");
    } catch (failure) { setError(ipcMessage(failure)); }
  };
  const sendFeedback = async () => {
    if (!current || !onSendFeedback || !current.threadKey || sending) return;
    setSending(true); setError(null);
    try {
      const saved = await saveDraft(current); if (!saved) return;
      const reserved = await api.previewOutput({ version: 2, action: "reserveFeedback", root: saved.snapshot.root, id: saved.id, revision: saved.revision });
      if (reserved.kind !== "review") throw new Error("Feedback could not be reserved; nothing was sent.");
      acceptReview(reserved.review);
      let delivery: "sent" | "unknown" | "unsent" = "unknown";
      try { delivery = await onSendFeedback(current.threadKey, outputFeedback(saved.snapshot, saved.anchor, saved.note)); }
      catch { /* An unclassified transport failure is unknown, never safe to resend. */ }
      const settled = await api.previewOutput({ version: 2, action: "settleFeedback", root: saved.snapshot.root,
        id: saved.id, revision: reserved.review.revision, delivery });
      if (settled.kind === "review") acceptReview(settled.review);
      setNotice(delivery === "sent" ? "Feedback sent to the selected thread." : delivery === "unknown"
        ? "Delivery is uncertain. Inspect the selected thread; this feedback will not be resent." : "Nothing was sent. Resolve the thread's error before trying again.");
    } catch (failure) { setError(ipcMessage(failure)); }
    finally { setSending(false); }
  };
  const closeOutput = (target: string) => {
    const remaining = outputs.filter(item => outputKey(item.snapshot.root, item.snapshot.path) !== target);
    setOutputs(remaining); setCloseDraft(null);
    if (root && key === target) {
      const next = remaining.filter(item => outputScopeRoot(item) === root).at(-1);
      setSelected(values => ({ ...values, [root]: next ? outputKey(next.snapshot.root, next.snapshot.path) : "" }));
      if (!next) setBrowse(true);
    }
  };
  const captureSelection = () => {
    const selection = window.getSelection();
    if (snapshot?.content.kind !== "text" || !selection || selection.rangeCount === 0 || !body.current) return;
    const range = selection.getRangeAt(0);
    if (!body.current.contains(range.startContainer) || !body.current.contains(range.endContainer)) return;
    const anchor = outputTextAnchor(snapshot.content.text, selection.toString());
    if (anchor) changeAnchor(anchor);
  };
  const openLatest = useRef(open);
  openLatest.current = open;
  const opened = useRef<number | null>(null);
  useEffect(() => {
    if (!root || openRequest === null || opened.current === openRequest.nonce) return;
    opened.current = openRequest.nonce;
    if (openRequest.directory) {
      setBrowse(true);
      void list(openRequest.path);
      return;
    }
    void openLatest.current(openRequest.path, openRequest);
  }, [root, openRequest, list]);
  const filtered = entries.filter(entry => entry.name.toLowerCase().includes(filter.toLowerCase()));
  const folder = snapshot?.path.includes("/") ? snapshot.path.slice(0, snapshot.path.lastIndexOf("/") + 1) : "";
  const reviewLocked = transitioning || sending || current?.saved?.delivery === "sending" || current?.saved?.delivery === "unknown";
  return <aside className={`output-preview${presenceClass(phase)}`} aria-label="Output preview" hidden={!root || !visible}
    inert={phase === "leaving" || transitioning || undefined} onAnimationEnd={onAnimationEnd}
    onKeyDown={event => { event.stopPropagation(); }}>
    {/* The agent overview's head: the open files are the tab row, then the
        panel's own actions. With nothing open, the row names the panel. */}
    <header className="output-preview-head">
      {projectOutputs.length > 0 ? <div ref={tabs} className="output-tabs" role="tablist" aria-label="Open outputs">
        {projectOutputs.map(item => {
          const target = outputKey(item.snapshot.root, item.snapshot.path);
          return <div key={target} className={target === key ? "output-tab is-active" : "output-tab"}>
            <button role="tab" type="button" aria-selected={target === key} title={item.snapshot.path}
              onClick={() => { if (root) setSelected(values => ({ ...values, [root]: target })); setBrowse(false); }}>
              <span className="codicon codicon-file" aria-hidden="true" />
              <span className="output-tab-name">{item.snapshot.path.split("/").at(-1)}</span>
              {item.note.trim() && <span className="output-tab-draft" role="img" aria-label="Has feedback draft" />}
            </button>
            <button type="button" className="output-tab-close" disabled={saving || transitioning} aria-label={`Close ${item.snapshot.path}`} onClick={() => {
              if (dirtyDraft(item)) setCloseDraft(target); else closeOutput(target);
            }}><span className="codicon codicon-close" aria-hidden="true" /></button>
          </div>;
        })}
      </div> : <div className="output-preview-title"><strong>Outputs</strong><span title={root ?? ""}>{root && lastSegment(root)}</span></div>}
      <button type="button" className={browse ? "terminal-center-action is-active" : "terminal-center-action"} title="Browse project files" aria-label="Browse project files" aria-pressed={browse}
        onClick={() => { setBrowse(value => !value); }}><span className="codicon codicon-folder" aria-hidden="true" /></button>
      <button type="button" className="terminal-center-action" title="Close" aria-label="Close output preview" onClick={closePanel}><span className="codicon codicon-close" aria-hidden="true" /></button>
    </header>
    {browse && <div className="output-browser">
      <form className="output-path" onSubmit={event => { event.preventDefault(); void open(path); }}>
        <input aria-label="Output file path" placeholder="Open a file by its project path…" value={path} onChange={event => { setPath(event.target.value); }} />
        <button type="submit" className="btn" disabled={!path.trim() || loading}>Open</button>
      </form>
      <div className="output-folder">
        <button type="button" className="terminal-center-action" title="Parent folder" disabled={!directory || listing} onClick={() => { void list(directory.split("/").slice(0, -1).join("/")); }} aria-label="Parent output folder"><span className="codicon codicon-chevron-up" aria-hidden="true" /></button>
        <span title={directory || "/"}>{directory || (root && lastSegment(root)) || "Project files"}</span>
        <input className="output-filter" aria-label="Filter output files" placeholder="Filter…" value={filter} onChange={event => { setFilter(event.target.value); }} />
        <button type="button" className="terminal-center-action" title="Refresh" disabled={listing} onClick={() => { void list(directory); }} aria-label="Refresh output files"><span className="codicon codicon-refresh" aria-hidden="true" /></button>
      </div>
      <div className="output-file-list" aria-label="Project files" aria-busy={listing}>
        {listing ? <p className="output-message">Reading files…</p> : filtered.slice(0, 200).map(entry =>
          <button type="button" key={entry.name} disabled={loading} title={entry.name}
            onClick={() => { const target = directory ? `${directory}/${entry.name}` : entry.name;
              if (entry.directory) void list(target); else void open(target); }}>
            <span className={`codicon codicon-${entry.directory ? "folder" : "file"}`} aria-hidden="true" />
            <span>{entry.name}</span>{entry.directory && <span className="codicon codicon-chevron-right" aria-hidden="true" />}
          </button>)}
        {!listing && filtered.length === 0 && <p className="output-message">No matching files.</p>}
        {filtered.length > 200 && <p className="output-message">Showing 200 of {filtered.length}. Filter to narrow the list.</p>}
      </div>
    </div>}
    {error && <p className="output-message is-error" role="alert">{error}</p>}
    {notice && <p className="output-message" role="status">{notice}</p>}
    {closeDraft && <div className="output-change"><span>This preview has a feedback draft.</span>
      <button type="button" className="btn" onClick={() => {
        const item = outputs.find(output => outputKey(output.snapshot.root, output.snapshot.path) === closeDraft);
        if (item) void transition(async () => { if (await saveDraft(item)) closeOutput(closeDraft); });
      }}>Save draft and close</button>
      <button type="button" className="btn" onClick={() => { closeOutput(closeDraft); }}>Discard unsaved changes</button>
      <button type="button" className="btn" onClick={() => { setCloseDraft(null); }}>Keep open</button></div>}
    {loading && <p className="output-message" role="status">Opening output…</p>}
    {snapshot && current && <>
      <div className="output-file-head">
        <div className="output-file-title" title={outputPath(snapshot)}>
          {folder && <span className="output-file-folder">{folder}</span>}<strong>{snapshot.path.slice(folder.length)}</strong>
        </div>
        <span className="output-file-meta">{Math.ceil(snapshot.byteLength / 1024).toString()} KB</span>
        {current.saved ? <AppSelect compact className="output-version" label="Saved output version" prefix="version"
          disabled={saving || transitioning || sending} value={current.saved.id}
          options={reviews.filter(item => item.snapshot.root === snapshot.root && item.snapshot.path === snapshot.path)
            .map(item => ({ value: item.id, label: `${item.snapshot.revision?.slice(0, 8) ?? ""}${item.note ? " · feedback" : ""}`,
              triggerLabel: item.snapshot.revision?.slice(0, 8) ?? "" }))}
          onChange={id => {
            const review = reviews.find(item => item.id === id);
            if (!review || !key) return;
            void transition(async () => { if (await saveDraft(current)) update(key, () => ({ snapshot: review.snapshot,
              anchor: review.anchor, note: review.note, saved: review, threadKey: review.threadKey, task: review.task })); });
          }} />
          : snapshot.revision && <span className="output-file-meta" title={snapshot.revision}>version {snapshot.revision.slice(0, 8)}</span>}
        {renderable !== null && <div className="output-mode" role="group" aria-label="Page view">
          <button type="button" aria-pressed={!rendered} onClick={() => { setRenderedKey(null); }}>Source</button>
          <button type="button" aria-pressed={rendered} onClick={() => { setRenderedKey(key ?? null); }}>Render</button>
        </div>}
        {snapshot.content.kind === "text" && snapshot.content.format === "markdown" && <div className="output-mode" role="group" aria-label="Markdown view">
          <button type="button" aria-pressed={!plain} onClick={() => { setPlain(false); }}>Preview</button>
          <button type="button" aria-pressed={plain} onClick={() => { setPlain(true); }}>Source</button>
        </div>}
        <button type="button" className="terminal-center-action" title={checking ? "Checking…" : "Check for changes"} aria-label="Check for changes"
          disabled={checking} onClick={() => { void check(); }}><span className={checking ? "codicon codicon-refresh codicon-modifier-spin" : "codicon codicon-refresh"} aria-hidden="true" /></button>
        {current.saved && <button type="button" className="terminal-center-action" title="Delete saved review" aria-label="Delete saved review"
          disabled={saving || sending} onClick={() => { if (!current.saved || !key) return;
            const saved = current.saved;
            void transition(async () => {
              await api.previewOutput({ version: 2, action: "deleteReview", root: snapshot.root, id: saved.id, revision: saved.revision });
              setReviews(items => items.filter(item => item.id !== saved.id)); closeOutput(key);
            });
          }}><span className="codicon codicon-trash" aria-hidden="true" /></button>}
      </div>
      {current.latest && <div className="output-change" role="status">
        <span>The file changed. Your preview and feedback still refer to version {snapshot.revision?.slice(0, 8) ?? "on screen"}.</span>
        <button type="button" className="btn" aria-pressed={compare} onClick={() => { setCompare(value => !value); }}>Compare versions</button>
        <button type="button" className="btn" disabled={saving || transitioning} onClick={() => { void transition(loadLatest); }}>Load latest</button>
      </div>}
      {compare && current.latest && <section className="output-comparison" aria-label="Compare output versions">
        {[{ title: "Reviewed", value: snapshot }, { title: "Current file", value: current.latest }].map(({ title, value }) =>
          <div key={title}><strong>{title} · {value.revision?.slice(0, 8)}</strong>
            {value.content.kind === "text" && <OutputSource text={value.content.text} diff={false} />}
            {value.content.kind === "image" && <ImagePreview snapshot={value} anchor={{ kind: "file" }} onAnchor={() => {}} />}
            {value.content.kind === "pdf" && <Suspense fallback={<p>Loading PDF…</p>}><PdfPreview bytes={value.content.bytes} initialPage={1} onAnnotate={() => {}} /></Suspense>}
            {value.content.kind === "unsupported" && <p>{value.content.reason}</p>}
          </div>)}
      </section>}
      <div className="output-content" hidden={compare} ref={body} onMouseUp={captureSelection} onKeyUp={captureSelection}>
        {snapshot.content.kind === "text" && (rendered && renderable !== null && api.visualFrame !== undefined
          ? <RenderedPagePreview text={snapshot.content.text} kind={renderable} frames={api.visualFrame}
            title={snapshot.path} />
          : snapshot.content.format === "markdown" && !plain
          ? <Markdown text={snapshot.content.text} />
          : <OutputSource text={snapshot.content.text} diff={snapshot.content.format === "diff"} />)}
        {snapshot.content.kind === "image" && <ImagePreview key={`${key}:${snapshot.revision}`} snapshot={snapshot} anchor={current.anchor} onAnchor={changeAnchor} />}
        {snapshot.content.kind === "pdf" && <Suspense fallback={<p className="output-message">Loading PDF viewer…</p>}>
          <PdfPreview key={`${key}:${snapshot.revision}`} bytes={snapshot.content.bytes} initialPage={current.anchor.kind === "page" ? current.anchor.page : 1} onAnnotate={annotatePage} />
        </Suspense>}
        {snapshot.content.kind === "unsupported" && <div className="output-unsupported"><span className="codicon codicon-file" aria-hidden="true" />
          <strong>Open in another app</strong><p>{snapshot.content.reason}</p>
          <div className="output-empty-actions">
            <button type="button" className="btn" onClick={() => { onOpenFolder(snapshot.root); }}>Open project in Finder</button>
            <button type="button" className="btn" onClick={() => { void onCopy(outputPath(snapshot)).then(() => { setNotice("File path copied."); }).catch(failure => { setError(ipcMessage(failure)); }); }}>Copy file path</button>
          </div>
        </div>}
      </div>
      {/* Feedback is a composer like the Threads one: the note, then a foot
          with where it goes and the verbs. */}
      {snapshot.revision && <div className="output-feedback">
        <div className="output-feedback-label"><strong>Feedback</strong><span>{outputAnchorLabel(current.anchor)}</span>
          {current.anchor.kind !== "file" && <button type="button" className="output-feedback-link" onClick={() => { changeAnchor({ kind: "file" }); }}>Clear selection</button>}
          <span className="output-feedback-status">{saving ? "Saving draft…" : current.saved && !dirtyDraft(current) ? "Draft saved on this Mac" : "Draft not saved yet"}</span>
          <button type="button" className="output-feedback-link" disabled={saving || sending} onClick={() => { setError(null); void saveDraft(current); }}>Save draft</button>
        </div>
        <div className="output-feedback-box">
          {current.anchor.kind === "text" && <blockquote>{current.anchor.quote}</blockquote>}
          <textarea aria-label="Output feedback" disabled={reviewLocked} placeholder="What should the agent change?" value={current.note} maxLength={8000}
            onChange={event => { if (key) update(key, item => ({ ...item, note: event.target.value })); setNotice(null); }} />
          <div className="output-feedback-foot">
            <AppSelect compact className="output-feedback-thread" label="Output feedback thread" placeholder="Choose a thread"
              disabled={reviewLocked} value={current.threadKey ?? ""}
              options={[
                ...(current.threadKey && !feedbackTargets.some(target => target.key === current.threadKey)
                  ? [{ value: current.threadKey, label: `Unavailable thread · ${current.threadKey}`, disabled: true }] : []),
                ...feedbackTargets.map(target => ({ value: target.key, label: target.title })),
              ]}
              onChange={value => { if (key) update(key, item => ({ ...item, threadKey: value || null })); }} />
            <input className="output-feedback-task" aria-label="Output review task" placeholder="Related task" maxLength={512} value={current.task ?? ""}
              disabled={reviewLocked}
              onChange={event => { if (key) update(key, item => ({ ...item, task: event.target.value })); }} />
            <button type="button" className="terminal-center-action" title="Copy feedback" aria-label="Copy feedback" disabled={!current.note.trim()} onClick={() => {
              void onCopy(outputFeedback(snapshot, current.anchor, current.note))
                .then(() => { setNotice("Feedback copied. Paste it into the agent pane when ready."); })
                .catch(failure => { setError(ipcMessage(failure)); });
            }}><span className="codicon codicon-copy" aria-hidden="true" /></button>
            {onSendFeedback && <button type="button" className="btn btn-primary output-feedback-send" disabled={sending || saving || !current.note.trim()
              || !feedbackTargets.some(target => target.key === current.threadKey)
              || current.saved?.delivery === "sending" || current.saved?.delivery === "sent" || current.saved?.delivery === "unknown"}
              onClick={() => { void sendFeedback(); }}>{sending ? "Sending…" : "Send feedback"}</button>}
          </div>
        </div>
        {(current.saved?.delivery === "sending" || current.saved?.delivery === "unknown") && <p className="output-feedback-warning" role="status">This feedback may have been delivered. Inspect the selected thread; it cannot be resent.</p>}
      </div>}
    </>}
    {!snapshot && !loading && <div className="output-empty"><span className="codicon codicon-file" aria-hidden="true" />
      <strong>Inspect what your agent made</strong><p>Choose Markdown, text, a diff, an image or a PDF. Select a passage, page or image region to draft feedback.</p>
      <span>Open previews stay on the version you reviewed until you load the latest file.</span>
    </div>}
  </aside>;
});
