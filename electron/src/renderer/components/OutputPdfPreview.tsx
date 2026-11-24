import { useEffect, useRef, useState } from "react";
import { AnnotationMode, getDocument, PDFWorker, type PDFDocumentProxy } from "pdfjs-dist";
import PdfWorker from "pdfjs-dist/build/pdf.worker.min.mjs?worker";

/** Canvas rendering only: no PDF scripting manager, forms, links or embedded files. */
export default function OutputPdfPreview({ bytes, initialPage = 1, onAnnotate }: {
  bytes: readonly number[]; initialPage?: number; onAnnotate: (page: number) => void;
}) {
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [page, setPage] = useState(initialPage);
  const [zoom, setZoom] = useState(1);
  const [error, setError] = useState<string | null>(null);
  const [rendering, setRendering] = useState(true);
  const [width, setWidth] = useState(500);
  const host = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    // Each width change re-renders the page into a new canvas, so a window
    // drag settles first: the first measurement applies at once, later ones
    // after a 120 ms pause, rounded to 8 px.
    let measured = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const apply = (): void => {
      const next = Math.max(100, Math.round((element.clientWidth - 24) / 8) * 8);
      setWidth(current => current === next ? current : next);
    };
    const observer = new ResizeObserver(() => {
      if (!measured) {
        measured = true;
        apply();
        return;
      }
      clearTimeout(timer);
      timer = setTimeout(apply, 120);
    });
    observer.observe(element);
    return () => { clearTimeout(timer); observer.disconnect(); };
  }, []);
  useEffect(() => {
    let live = true;
    const port = new PdfWorker();
    const worker = PDFWorker.create({ port });
    const loading = getDocument({ data: new Uint8Array(bytes), worker,
      useWorkerFetch: false, disableFontFace: true, useSystemFonts: true,
      maxImageSize: 16_000_000, canvasMaxAreaInBytes: 32_000_000,
      isOffscreenCanvasSupported: false, isImageDecoderSupported: false,
      stopAtErrors: true });
    loading.onPassword = () => { if (live) setError("This PDF is password protected. Open it in its own application."); };
    void loading.promise.then(value => { if (live) { setDocument(value); setPage(n => Math.min(n, value.numPages)); } })
      .catch(() => { if (live) setError("This PDF could not be rendered. Open it in its own application."); });
    return () => { live = false; void loading.destroy().finally(() => { worker.destroy(); port.terminate(); }); };
  }, [bytes]);
  useEffect(() => {
    if (!document || !canvas.current) return;
    let live = true;
    let task: ReturnType<Awaited<ReturnType<PDFDocumentProxy["getPage"]>>["render"]> | undefined;
    setRendering(true); setError(null);
    void document.getPage(page).then(pdfPage => {
      const target = canvas.current;
      if (!live || !target) return;
      const original = pdfPage.getViewport({ scale: 1 });
      const scale = Math.min(width / original.width * zoom, 2400 / original.width, 3200 / original.height);
      const viewport = pdfPage.getViewport({ scale });
      const density = Math.min(window.devicePixelRatio || 1, 2);
      target.width = Math.ceil(viewport.width * density);
      target.height = Math.ceil(viewport.height * density);
      target.style.width = `${viewport.width.toString()}px`;
      target.style.height = `${viewport.height.toString()}px`;
      task = pdfPage.render({ canvas: target, viewport,
        transform: density === 1 ? undefined : [density, 0, 0, density, 0, 0],
        annotationMode: AnnotationMode.DISABLE });
      return task.promise;
    }).then(() => { if (live) setRendering(false); }).catch(() => {
      if (live) { setRendering(false); setError("This page could not be rendered."); }
    });
    return () => { live = false; task?.cancel(); };
  }, [document, page, zoom, width]);
  return <div className="output-pdf" ref={host}>
    <div className="output-pdf-controls">
      <button type="button" disabled={!document || page <= 1} onClick={() => { setPage(n => n - 1); }} aria-label="Previous PDF page">‹</button>
      <label>Page <input aria-label="PDF page" type="number" min={1} max={document?.numPages ?? 1} value={page}
        onChange={event => { const n = Number(event.target.value); if (Number.isInteger(n) && n >= 1 && n <= (document?.numPages ?? 1)) setPage(n); }} /></label>
      <span>of {document?.numPages ?? "…"}</span>
      <button type="button" disabled={!document || page >= document.numPages} onClick={() => { setPage(n => n + 1); }} aria-label="Next PDF page">›</button>
      <button type="button" disabled={!document || rendering || !!error} onClick={() => { onAnnotate(page); }}>Comment on page</button>
      <select aria-label="PDF zoom" value={zoom} onChange={event => { setZoom(Number(event.target.value)); }}>
        <option value={1}>Fit width</option><option value={1.5}>150%</option><option value={2}>200%</option>
      </select>
    </div>
    {error && <p role="alert" className="output-message">{error}</p>}
    {rendering && !error && <span role="status" className="output-pdf-loading">Rendering page…</span>}
    <div className="output-pdf-page" hidden={!!error}><canvas ref={canvas} aria-label={`PDF page ${page.toString()}`} /></div>
  </div>;
}
